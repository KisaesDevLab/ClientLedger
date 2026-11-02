//! Sales Tax Setup (P23 plan §3 WP-2, `lists.sales_tax`; U-147, U-25):
//! three tabs — taxes and groups, agencies, tax codes — each a list with
//! an editor card. Deactivate, never delete. A legacy tax (made from an
//! old line rate, Q-P23f) shows the "choose its agency" banner; an edit of
//! a tax already on documents shows the new-documents-only note (Q-P23v).
//!
//! Every rate shown is the engine's (`TaxRate::format_percent`); the
//! editors check what was typed (`cl_model::forms::sales_tax::setup`) and
//! send it. Writes carry one idempotency key per save attempt.

use std::sync::Arc;

use cl_api::{
    ApiError, Capabilities, Contact, ContactFilter, ContactType, IdempotencyKey, OrphanLabel,
    RequestGroup, SalesTaxAgency, SalesTaxItem, SalesTaxItemDraft, SalesTaxKind, SalesTaxRefusal,
    TaxCode, TaxCodeDraft,
};
use cl_core::Timestamp;
use cl_core::ids::{ContactId, SalesTaxAgencyId, SalesTaxItemId, TaxCodeId};
use cl_model::forms::sales_tax::setup::{
    CodeEditor, GroupEditor, ItemEditor, ItemPlan, SetupProblem,
};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::i18n::t;
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "lists.sales_tax";

/// The tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    /// Taxes and groups.
    Items,
    /// Agencies.
    Agencies,
    /// Tax codes.
    Codes,
}

impl Tab {
    const fn wire(self) -> &'static str {
        match self {
            Tab::Items => "items",
            Tab::Agencies => "agencies",
            Tab::Codes => "codes",
        }
    }
}

/// The open editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Editor {
    /// A tax (`id: None`: new).
    Item {
        id: Option<SalesTaxItemId>,
        ed: ItemEditor,
        saved: ItemEditor,
        version: Option<Timestamp>,
    },
    /// A group.
    Group {
        id: Option<SalesTaxItemId>,
        ed: GroupEditor,
        version: Option<Timestamp>,
    },
    /// A new agency (a vendor).
    Agency { vendor: Option<ContactId> },
    /// A code.
    Code {
        id: Option<TaxCodeId>,
        ed: CodeEditor,
        version: Option<Timestamp>,
    },
}

/// What a save answers.
enum Saved {
    Item(SalesTaxItem),
    Agency(SalesTaxAgency),
    Code(TaxCode),
}

/// The Sales Tax Setup window.
pub struct SetupScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    available: bool,
    pub(crate) tab: Tab,
    pub(crate) show_inactive: bool,
    items: Vec<SalesTaxItem>,
    agencies: Vec<SalesTaxAgency>,
    codes: Vec<TaxCode>,
    vendors: Vec<Contact>,
    items_job: Option<Job<Vec<SalesTaxItem>>>,
    agencies_job: Option<Job<Vec<SalesTaxAgency>>>,
    codes_job: Option<Job<Vec<TaxCode>>>,
    vendors_job: Option<Job<Vec<Contact>>>,
    save_job: Option<Job<Saved>>,
    /// The key of the save attempt in flight (reused if it is retried).
    key: Option<IdempotencyKey>,
    pub(crate) editor: Option<Editor>,
    pub(crate) selected: Option<String>,
    problem: Option<&'static str>,
    error: Option<ApiError>,
    saved: u32,
    loaded: bool,
}

fn refusal_key(e: &ApiError) -> Option<&'static str> {
    Some(match cl_api::sales_tax_refusal(e)? {
        SalesTaxRefusal::Stale => "sales_tax.refusal.stale",
        SalesTaxRefusal::AgencyExists => "sales_tax.refusal.agency_exists",
        SalesTaxRefusal::AgencyInUse => "sales_tax.refusal.agency_in_use",
        SalesTaxRefusal::NameInUse => "sales_tax.refusal.name_in_use",
        SalesTaxRefusal::ItemInGroup => "sales_tax.refusal.item_in_group",
        SalesTaxRefusal::CodeInUse => "sales_tax.refusal.code_in_use",
        SalesTaxRefusal::SystemCode => "sales_tax.refusal.system_code",
        _ => "sales_tax.refusal.other",
    })
}

impl SetupScreen {
    /// The window over the open company.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>) -> SetupScreen {
        let group = link.group().and_then(Result::ok);
        let available = link.is_demo() || link.capabilities().has(Capabilities::SALES_TAX_SETUP);
        let mut s = SetupScreen {
            link,
            group,
            available,
            tab: Tab::Items,
            show_inactive: false,
            items: Vec::new(),
            agencies: Vec::new(),
            codes: Vec::new(),
            vendors: Vec::new(),
            items_job: None,
            agencies_job: None,
            codes_job: None,
            vendors_job: None,
            save_job: None,
            key: None,
            editor: None,
            selected: None,
            problem: None,
            error: None,
            saved: 0,
            loaded: false,
        };
        s.refresh();
        s
    }

    fn refresh(&mut self) {
        if !self.available {
            return;
        }
        let all = self.show_inactive;
        self.items_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| Ok(crate::company::demo::sales_tax::items(d, all)),
            move |s| async move { s.sales_tax().items(all).await },
        ));
        self.agencies_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| Ok(crate::company::demo::sales_tax::agencies(d, all)),
            move |s| async move { s.sales_tax().agencies(all).await },
        ));
        self.codes_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| Ok(crate::company::demo::sales_tax::codes(d, all)),
            move |s| async move { s.sales_tax().codes(all).await },
        ));
        self.vendors_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| {
                Ok(d.contacts_mut()
                    .iter()
                    .filter(|c| {
                        c.is_active
                            && matches!(
                                c.fields.contact_type,
                                ContactType::Vendor | ContactType::Both
                            )
                    })
                    .cloned()
                    .collect())
            },
            |s| async move {
                let f = ContactFilter {
                    contact_type: Some(ContactType::Vendor),
                    is_active: Some(true),
                    ..ContactFilter::default()
                };
                s.contacts().list_all(&f).await
            },
        ));
        self.poll();
    }

    fn busy(&self) -> bool {
        self.items_job.is_some()
            || self.agencies_job.is_some()
            || self.codes_job.is_some()
            || self.vendors_job.is_some()
            || self.save_job.is_some()
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.items_job) {
            match r {
                Ok(v) => {
                    self.items = v;
                    self.loaded = true;
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.agencies_job) {
            match r {
                Ok(v) => self.agencies = v,
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.codes_job) {
            match r {
                Ok(v) => self.codes = v,
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.vendors_job) {
            self.vendors = r.unwrap_or_default();
        }
        if let Some(r) = engine::poll(&mut self.save_job) {
            self.key = None;
            match r {
                Ok(s) => {
                    self.selected = Some(match s {
                        Saved::Item(i) => i.name,
                        Saved::Agency(a) => a.vendor_name,
                        Saved::Code(c) => c.code,
                    });
                    self.saved += 1;
                    self.editor = None;
                    self.problem = None;
                    crate::forms::publish_saved(&self.link, &self.link.bus().subscribe(), &[]);
                    self.refresh();
                }
                Err(e) => match refusal_key(&e) {
                    Some(k) => self.problem = Some(k),
                    None => self.error = Some(e),
                },
            }
        }
    }

    // ---- Editors ---------------------------------------------------------------

    /// Ctrl+N: a new entry on the tab in front.
    pub(crate) fn new_entry(&mut self) {
        self.problem = None;
        self.editor = Some(match self.tab {
            Tab::Items => Editor::Item {
                id: None,
                ed: ItemEditor::default(),
                saved: ItemEditor::default(),
                version: None,
            },
            Tab::Agencies => Editor::Agency { vendor: None },
            Tab::Codes => Editor::Code {
                id: None,
                ed: CodeEditor::default(),
                version: None,
            },
        });
    }

    pub(crate) fn new_group(&mut self) {
        self.problem = None;
        self.editor = Some(Editor::Group {
            id: None,
            ed: GroupEditor::default(),
            version: None,
        });
    }

    fn open_item(&mut self, id: SalesTaxItemId) {
        let Some(i) = self.items.iter().find(|i| i.id == id) else {
            return;
        };
        self.problem = None;
        self.selected = Some(i.name.clone());
        self.editor = Some(if i.kind == SalesTaxKind::Group {
            let members: Vec<SalesTaxItemId> = i.members.iter().map(|m| m.id).collect();
            let mut ed = GroupEditor::new(&i.name, &members);
            ed.description = i.description.clone().unwrap_or_default();
            Editor::Group {
                id: Some(id),
                ed,
                version: i.updated_at,
            }
        } else {
            let ed = ItemEditor::from_saved(
                &i.name,
                i.description.as_deref(),
                i.rate,
                i.agency_id,
                i.in_use,
            );
            Editor::Item {
                id: Some(id),
                saved: ed.clone(),
                ed,
                version: i.updated_at,
            }
        });
    }

    fn open_code(&mut self, id: TaxCodeId) {
        let Some(c) = self.codes.iter().find(|c| c.id == id) else {
            return;
        };
        self.problem = None;
        self.selected = Some(c.code.clone());
        self.editor = Some(Editor::Code {
            id: Some(id),
            ed: CodeEditor {
                code: c.code.clone(),
                name: c.name.clone(),
                taxable: c.taxable,
                is_system: c.is_system,
            },
            version: c.updated_at,
        });
    }

    fn setup_problem(&mut self, p: SetupProblem) {
        self.problem = Some(p.key().0);
    }

    pub(crate) fn save(&mut self) {
        let Some(ed) = self.editor.clone() else {
            return;
        };
        let key = self.key.get_or_insert_with(IdempotencyKey::new).clone();
        let label = OrphanLabel {
            what: "orphan.sales_tax_setup",
            key: Some(key.clone()),
        };
        let job = match ed {
            Editor::Item {
                id, ed, version, ..
            } => match ed.plan() {
                Ok(p) => self.save_item(id, &p, version, key, label),
                Err(p) => return self.setup_problem(p),
            },
            Editor::Group { id, ed, version } => match ed.plan() {
                Ok(p) => self.save_item(id, &p, version, key, label),
                Err(p) => return self.setup_problem(p),
            },
            Editor::Agency { vendor } => {
                let Some(vendor) = vendor else {
                    self.problem = Some("sales_tax.setup.pick_vendor");
                    return;
                };
                engine::write(
                    &self.link,
                    self.group.as_ref(),
                    label,
                    move |d| {
                        crate::company::demo::sales_tax::create_agency(d, vendor).map(Saved::Agency)
                    },
                    move |s| async move {
                        s.sales_tax()
                            .create_agency(vendor, key)
                            .await
                            .map(Saved::Agency)
                    },
                )
            }
            Editor::Code { id, ed, version } => match ed.plan() {
                Ok(None) => {
                    self.editor = None;
                    return;
                }
                Ok(Some(p)) => {
                    let draft = TaxCodeDraft {
                        code: p.code,
                        name: p.name,
                        taxable: p.taxable,
                    };
                    let d2 = draft.clone();
                    engine::write(
                        &self.link,
                        self.group.as_ref(),
                        label,
                        move |d| {
                            crate::company::demo::sales_tax::save_code(d, id, &d2).map(Saved::Code)
                        },
                        move |s| async move {
                            let st = s.sales_tax();
                            match id {
                                Some(id) => st.update_code(id, &draft, version, key).await,
                                None => st.create_code(&draft, key).await,
                            }
                            .map(Saved::Code)
                        },
                    )
                }
                Err(p) => return self.setup_problem(p),
            },
        };
        self.problem = None;
        self.save_job = Some(job);
        self.poll();
    }

    fn save_item(
        &self,
        id: Option<SalesTaxItemId>,
        plan: &ItemPlan,
        version: Option<Timestamp>,
        key: IdempotencyKey,
        label: OrphanLabel,
    ) -> Job<Saved> {
        let draft = match plan.clone() {
            ItemPlan::Item {
                name,
                description,
                rate,
                agency,
            } => SalesTaxItemDraft {
                kind: SalesTaxKind::Item,
                name,
                description,
                rate: Some(rate),
                agency_id: Some(agency),
                member_ids: Vec::new(),
            },
            ItemPlan::Group {
                name,
                description,
                members,
            } => SalesTaxItemDraft {
                kind: SalesTaxKind::Group,
                name,
                description,
                rate: None,
                agency_id: None,
                member_ids: members,
            },
        };
        let d2 = draft.clone();
        engine::write(
            &self.link,
            self.group.as_ref(),
            label,
            move |d| {
                crate::company::demo::sales_tax::save_item(d, id, &d2, version).map(Saved::Item)
            },
            move |s| async move {
                let st = s.sales_tax();
                match id {
                    Some(id) => st.update_item(id, &draft, version, key).await,
                    None => st.create_item(&draft, key).await,
                }
                .map(Saved::Item)
            },
        )
    }

    /// Ctrl+Shift+I: make the open entry inactive or active again.
    pub(crate) fn toggle_active(&mut self) {
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.sales_tax_setup",
            key: Some(key.clone()),
        };
        let job = match self.editor.clone() {
            Some(Editor::Item { id: Some(id), .. } | Editor::Group { id: Some(id), .. }) => {
                let active = !self.items.iter().any(|i| i.id == id && i.is_active);
                engine::write(
                    &self.link,
                    self.group.as_ref(),
                    label,
                    move |d| {
                        crate::company::demo::sales_tax::set_item_active(d, id, active)
                            .map(Saved::Item)
                    },
                    move |s| async move {
                        s.sales_tax()
                            .set_item_active(id, active, key)
                            .await
                            .map(Saved::Item)
                    },
                )
            }
            Some(Editor::Code { id: Some(id), .. }) => engine::write(
                &self.link,
                self.group.as_ref(),
                label,
                move |d| crate::company::demo::sales_tax::deactivate_code(d, id).map(Saved::Code),
                move |s| async move {
                    s.sales_tax()
                        .deactivate_code(id, key)
                        .await
                        .map(Saved::Code)
                },
            ),
            _ => return,
        };
        self.save_job = Some(job);
        self.poll();
    }

    fn set_agency_active(&mut self, id: SalesTaxAgencyId, active: bool) {
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.sales_tax_setup",
            key: Some(key.clone()),
        };
        self.save_job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label,
            move |d| {
                crate::company::demo::sales_tax::set_agency_active(d, id, active).map(Saved::Agency)
            },
            move |s| async move {
                s.sales_tax()
                    .set_agency_active(id, active, key)
                    .await
                    .map(Saved::Agency)
            },
        ));
        self.poll();
    }

    // ---- Drawing ----------------------------------------------------------------

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        ui.label(
            RichText::new(t("sales_tax.setup.title"))
                .size(22.0)
                .strong(),
        );
        if !self.available {
            form::hint(ui, &tokens, &t("sales_tax.setup.needs_update"));
            return;
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        if !self.loaded && self.error.is_none() {
            form::progress_line(ui, &tokens, &t("sales_tax.setup.loading"));
        }
        if self.items.iter().any(SalesTaxItem::needs_agency) {
            form::inline_warning(ui, &tokens, &t("sales_tax.setup.legacy_banner"));
        }
        ui.horizontal(|ui| {
            for (tab, key) in [
                (Tab::Items, "sales_tax.setup.tab.items"),
                (Tab::Agencies, "sales_tax.setup.tab.agencies"),
                (Tab::Codes, "sales_tax.setup.tab.codes"),
            ] {
                if ui.selectable_label(self.tab == tab, t(key)).clicked() && self.tab != tab {
                    self.tab = tab;
                    self.editor = None;
                    self.problem = None;
                }
            }
            ui.separator();
            let mut all = self.show_inactive;
            if ui
                .checkbox(&mut all, t("sales_tax.setup.show_inactive"))
                .changed()
            {
                self.show_inactive = all;
                self.refresh();
            }
        });
        ui.add_space(6.0);
        match self.tab {
            Tab::Items => self.items_ui(ui, &tokens),
            Tab::Agencies => self.agencies_ui(ui, &tokens),
            Tab::Codes => self.codes_ui(ui, &tokens),
        }
        ui.add_space(10.0);
        self.editor_ui(ui, &tokens);
    }

    fn status(active: bool) -> String {
        if active {
            String::new()
        } else {
            t("sales_tax.setup.inactive")
        }
    }

    fn items_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.horizontal(|ui| {
            if ui.button(t("sales_tax.setup.new_item")).clicked() {
                self.new_entry();
            }
            if ui.button(t("sales_tax.setup.new_group")).clicked() {
                self.new_group();
            }
        });
        if self.loaded && self.items.is_empty() {
            ui.label(RichText::new(t("sales_tax.setup.empty_items")).color(tokens.text_secondary));
            return;
        }
        let mut open = None;
        egui::Grid::new("sales_tax/items")
            .striped(true)
            .show(ui, |ui| {
                for k in [
                    "sales_tax.setup.col.name",
                    "sales_tax.setup.col.kind",
                    "sales_tax.setup.col.rate",
                    "sales_tax.setup.col.agency",
                    "sales_tax.setup.col.members",
                    "sales_tax.setup.col.status",
                ] {
                    ui.strong(t(k));
                }
                ui.end_row();
                for i in &self.items {
                    if ui.link(&i.name).clicked() {
                        open = Some(i.id);
                    }
                    ui.label(t(if i.kind == SalesTaxKind::Group {
                        "sales_tax.setup.kind.group"
                    } else {
                        "sales_tax.setup.kind.item"
                    }));
                    ui.label(RichText::new(i.rate.format_percent()).monospace());
                    if i.needs_agency() {
                        ui.label(
                            RichText::new(t("sales_tax.setup.legacy")).color(tokens.warn_text),
                        );
                    } else {
                        ui.label(i.agency_name.clone().unwrap_or_default());
                    }
                    ui.label(
                        i.members
                            .iter()
                            .filter_map(|m| m.name.clone())
                            .collect::<Vec<_>>()
                            .join(" + "),
                    );
                    ui.label(Self::status(i.is_active));
                    ui.end_row();
                }
            });
        if let Some(id) = open {
            self.open_item(id);
        }
    }

    fn agencies_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if ui.button(t("sales_tax.setup.new_agency")).clicked() {
            self.new_entry();
        }
        if self.loaded && self.agencies.is_empty() {
            ui.label(
                RichText::new(t("sales_tax.setup.empty_agencies")).color(tokens.text_secondary),
            );
            return;
        }
        let mut toggle = None;
        egui::Grid::new("sales_tax/agencies")
            .striped(true)
            .show(ui, |ui| {
                ui.strong(t("sales_tax.setup.col.vendor"));
                ui.strong(t("sales_tax.setup.col.status"));
                ui.label("");
                ui.end_row();
                for a in &self.agencies {
                    ui.label(&a.vendor_name);
                    ui.label(Self::status(a.is_active));
                    let k = if a.is_active {
                        "sales_tax.setup.deactivate"
                    } else {
                        "sales_tax.setup.reactivate"
                    };
                    let r = ui.button(t(k));
                    // Each row's button names its agency for screen readers.
                    let r = r.on_hover_text(&a.vendor_name);
                    if r.clicked() {
                        toggle = Some((a.id, !a.is_active));
                    }
                    ui.end_row();
                }
            });
        if let Some((id, active)) = toggle {
            self.set_agency_active(id, active);
        }
    }

    fn codes_ui(&mut self, ui: &mut Ui, _tokens: &Tokens) {
        if ui.button(t("sales_tax.setup.new_code")).clicked() {
            self.new_entry();
        }
        let mut open = None;
        egui::Grid::new("sales_tax/codes")
            .striped(true)
            .show(ui, |ui| {
                for k in [
                    "sales_tax.setup.col.code",
                    "sales_tax.setup.col.name",
                    "sales_tax.setup.col.taxable",
                    "sales_tax.setup.col.status",
                ] {
                    ui.strong(t(k));
                }
                ui.end_row();
                for c in &self.codes {
                    if ui.link(&c.code).clicked() {
                        open = Some(c.id);
                    }
                    ui.label(&c.name);
                    ui.label(t(if c.taxable {
                        "sales_tax.setup.yes"
                    } else {
                        "sales_tax.setup.no"
                    }));
                    ui.label(if c.is_system {
                        t("sales_tax.setup.system")
                    } else {
                        Self::status(c.is_active)
                    });
                    ui.end_row();
                }
            });
        if let Some(id) = open {
            self.open_code(id);
        }
    }

    #[allow(clippy::too_many_lines)] // one card per editor kind
    fn editor_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let taxes: Vec<(SalesTaxItemId, String)> = self
            .items
            .iter()
            .filter(|i| i.kind == SalesTaxKind::Item && i.is_active)
            .map(|i| (i.id, i.name.clone()))
            .collect();
        let agencies: Vec<(SalesTaxAgencyId, String)> = self
            .agencies
            .iter()
            .filter(|a| a.is_active)
            .map(|a| (a.id, a.vendor_name.clone()))
            .collect();
        let vendors: Vec<(ContactId, String)> = self
            .vendors
            .iter()
            .filter(|v| !self.agencies.iter().any(|a| a.vendor_id == v.id))
            .map(|v| (v.id, v.fields.display_name.clone()))
            .collect();
        let active_items: Vec<SalesTaxItemId> = self
            .items
            .iter()
            .filter(|i| i.is_active)
            .map(|i| i.id)
            .collect();
        let active_codes: Vec<TaxCodeId> = self
            .codes
            .iter()
            .filter(|c| c.is_active)
            .map(|c| c.id)
            .collect();
        let all_agencies: Vec<(SalesTaxAgencyId, String)> = self
            .agencies
            .iter()
            .map(|a| (a.id, a.vendor_name.clone()))
            .collect();
        let active_now =
            |id: Option<SalesTaxItemId>| id.is_some_and(|id| active_items.contains(&id));
        let code_active = |id: Option<TaxCodeId>| id.is_some_and(|id| active_codes.contains(&id));
        let agency_label = |id: Option<SalesTaxAgencyId>| {
            all_agencies
                .iter()
                .find(|(a, _)| Some(*a) == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_default()
        };
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let mut save = false;
        let mut close = false;
        let mut toggle = false;
        form::card(ui, tokens, |ui| match editor {
            Editor::Item { id, ed, saved, .. } => {
                form::heading(
                    ui,
                    tokens,
                    &t(if id.is_some() {
                        "sales_tax.setup.edit"
                    } else {
                        "sales_tax.setup.new_item"
                    }),
                );
                form::field(ui, &t("sales_tax.setup.field.name"), &mut ed.name);
                form::field(
                    ui,
                    &t("sales_tax.setup.field.description"),
                    &mut ed.description,
                );
                form::field(ui, &t("sales_tax.setup.field.rate"), &mut ed.percent);
                form::hint(ui, tokens, &t("sales_tax.setup.rate_hint"));
                ui.horizontal(|ui| {
                    let l = ui.label(t("sales_tax.setup.field.agency"));
                    let shown = ed.agency.map_or_else(
                        || t("sales_tax.setup.pick_agency"),
                        |a| agency_label(Some(a)),
                    );
                    egui::ComboBox::from_id_salt("sales_tax/agency")
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            for (aid, name) in &agencies {
                                if ui.selectable_label(ed.agency == Some(*aid), name).clicked() {
                                    ed.agency = Some(*aid);
                                }
                            }
                        })
                        .response
                        .labelled_by(l.id);
                });
                if ed.in_use {
                    let note_on = ed.shows_forward_note(saved);
                    if note_on {
                        form::inline_warning(ui, tokens, &t("sales_tax.setup.in_use_note"));
                    }
                }
                buttons(
                    ui,
                    id.is_some(),
                    active_now(*id),
                    &mut save,
                    &mut toggle,
                    &mut close,
                    tokens,
                );
            }
            Editor::Group { id, ed, .. } => {
                form::heading(
                    ui,
                    tokens,
                    &t(if id.is_some() {
                        "sales_tax.setup.edit"
                    } else {
                        "sales_tax.setup.new_group"
                    }),
                );
                form::field(ui, &t("sales_tax.setup.field.name"), &mut ed.name);
                form::field(
                    ui,
                    &t("sales_tax.setup.field.description"),
                    &mut ed.description,
                );
                form::hint(ui, tokens, &t("sales_tax.setup.group_hint"));
                ui.label(t("sales_tax.setup.field.members"));
                for (tid, name) in &taxes {
                    let mut on = ed.members().contains(tid);
                    if ui.checkbox(&mut on, name).changed() {
                        if on {
                            ed.add(*tid);
                        } else {
                            ed.remove(*tid);
                        }
                    }
                }
                buttons(
                    ui,
                    id.is_some(),
                    active_now(*id),
                    &mut save,
                    &mut toggle,
                    &mut close,
                    tokens,
                );
            }
            Editor::Agency { vendor } => {
                form::heading(ui, tokens, &t("sales_tax.setup.new_agency"));
                ui.horizontal(|ui| {
                    let l = ui.label(t("sales_tax.setup.field.vendor"));
                    let shown = vendor
                        .and_then(|v| vendors.iter().find(|(x, _)| *x == v))
                        .map_or_else(|| t("sales_tax.setup.pick_vendor"), |(_, n)| n.clone());
                    egui::ComboBox::from_id_salt("sales_tax/vendor")
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            for (vid, name) in &vendors {
                                if ui.selectable_label(*vendor == Some(*vid), name).clicked() {
                                    *vendor = Some(*vid);
                                }
                            }
                        })
                        .response
                        .labelled_by(l.id);
                });
                buttons(ui, false, false, &mut save, &mut toggle, &mut close, tokens);
            }
            Editor::Code { id, ed, .. } => {
                form::heading(
                    ui,
                    tokens,
                    &t(if id.is_some() {
                        "sales_tax.setup.edit"
                    } else {
                        "sales_tax.setup.new_code"
                    }),
                );
                ui.add_enabled_ui(!ed.is_system, |ui| {
                    form::field(ui, &t("sales_tax.setup.field.code"), &mut ed.code);
                    form::field(ui, &t("sales_tax.setup.field.code_name"), &mut ed.name);
                    ui.checkbox(&mut ed.taxable, t("sales_tax.setup.field.taxable"));
                });
                let can_toggle = !ed.is_system && code_active(*id);
                buttons(
                    ui,
                    id.is_some() && !ed.is_system,
                    can_toggle,
                    &mut save,
                    &mut toggle,
                    &mut close,
                    tokens,
                );
            }
        });
        if let Some(p) = self.problem {
            ui.label(RichText::new(t(p)).color(tokens.warn_text));
        }
        if save {
            self.save();
        } else if toggle {
            self.toggle_active();
        } else if close {
            self.editor = None;
            self.problem = None;
        }
    }

    fn state_json(&self) -> Value {
        let editing = self.editor.as_ref().map(|e| match e {
            Editor::Item { id, .. } => json!({"kind": "item", "new": id.is_none()}),
            Editor::Group { id, ed, .. } => {
                json!({"kind": "group", "new": id.is_none(), "members": ed.members().len()})
            }
            Editor::Agency { vendor } => json!({"kind": "agency", "vendor": vendor.is_some()}),
            Editor::Code { id, ed, .. } => {
                json!({"kind": "code", "new": id.is_none(), "system": ed.is_system})
            }
        });
        let note = matches!(
            &self.editor,
            Some(Editor::Item { ed, saved, .. }) if ed.shows_forward_note(saved)
        );
        json!({
            "screen": "sales_tax_setup",
            "available": self.available,
            "tab": self.tab.wire(),
            "show_inactive": self.show_inactive,
            "items": self.items.len(),
            "groups": self.items.iter().filter(|i| i.kind == SalesTaxKind::Group).count(),
            "inactive_items": self.items.iter().filter(|i| !i.is_active).count(),
            "agencies": self.agencies.len(),
            "codes": self.codes.len(),
            "legacy": self.items.iter().any(SalesTaxItem::needs_agency),
            "editing": editing,
            "forward_note": note,
            "problem": self.problem,
            "saved": self.saved,
            "error": self.error.is_some(),
        })
    }
}

/// Save, Make inactive/active (an existing entry) and Cancel.
fn buttons(
    ui: &mut Ui,
    existing: bool,
    active: bool,
    save: &mut bool,
    toggle: &mut bool,
    close: &mut bool,
    tokens: &Tokens,
) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        *save = crate::widgets::primary_button(ui, &t("sales_tax.setup.save")).clicked();
        if existing {
            let k = if active {
                "sales_tax.setup.deactivate"
            } else {
                "sales_tax.setup.reactivate"
            };
            *toggle = ui
                .button(RichText::new(t(k)).color(tokens.warn_text))
                .clicked();
        }
        *close = ui.button(t("sales_tax.setup.cancel")).clicked();
    });
}

impl Screen for SetupScreen {
    fn title(&self) -> String {
        t("sales_tax.setup.title")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        egui::ScrollArea::vertical().show(ui, |ui| self.draw(ui));
        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        Scope::Window
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            KIND => true,
            "list.new" => {
                self.new_entry();
                true
            }
            "list.toggle_active" => {
                self.toggle_active();
                true
            }
            "list.show_inactive" => {
                self.show_inactive = !self.show_inactive;
                self.refresh();
                true
            }
            // Demo only: the seed again (the scenario's second theme run).
            "dev.demo_sales_tax_reset" => {
                let Some(mut d) = self.link.demo_state() else {
                    return false;
                };
                crate::company::demo::sales_tax::reset(&mut d);
                drop(d);
                self.editor = None;
                self.problem = None;
                self.show_inactive = false;
                self.tab = Tab::Items;
                self.refresh();
                true
            }
            _ => false,
        }
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
