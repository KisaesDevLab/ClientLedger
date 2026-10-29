//! Company > Company Information (P19, `company.information`; no mockup,
//! Q-P19e): what prints on forms and checks. The logo (Browse for a PNG or
//! JPEG up to 2 MB, preview, Remove; Q-P19v, Q-P19an), the company footer
//! with an override per form type (Q-P19l, U-111), and the numbering of
//! each type (prefix and next number; U-111). Numbers a live document
//! already holds are skipped by the engine when it numbers a new one.
//!
//! Engine calls go through the request queue; the demo answers at once.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use cl_api::{
    ApiError, Capabilities, DocType, DocumentNumber, DocumentNumberUpdate, FormFooters,
    MAX_FOOTER_CHARS, OrphanLabel, RequestGroup,
};
use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::CommandId;
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task};
use crate::i18n::t;
use crate::screens::card_charge::kit;
use crate::screens::company::form;
use crate::theme::Tokens;
use crate::widgets::pdf_viewer::{DocSource, DocViewer, ViewerState};
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "company.information";

/// The Company Information window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    _today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(CompanyInfoScreen::new(Arc::clone(link), open)) as Box<_>)
}

/// True when the owner removed the logo for `tenant` (Q-P19an).
#[must_use]
pub fn logo_hidden(link: &CompanyLink, tenant: TenantId) -> bool {
    link.store()
        .load()
        .ok()
        .and_then(|l| l.get(tenant).map(|r| r.prefs.hide_logo))
        .unwrap_or(false)
}

/// What the window reads.
#[derive(Clone, Default)]
pub(crate) struct Loaded {
    logo: Option<Vec<u8>>,
    footers: FormFooters,
    numbers: Vec<DocumentNumber>,
}

/// The four form types' labels.
fn type_key(t: DocType) -> &'static str {
    match t {
        DocType::Invoice => "print.kind.invoice",
        DocType::SalesReceipt => "print.kind.sales_receipt",
        DocType::CreditMemo => "print.kind.credit_memo",
        DocType::Statement => "print.kind.statement",
    }
}

/// The Company Information window.
pub struct CompanyInfoScreen {
    link: Arc<CompanyLink>,
    tenant: Option<(TenantId, String)>,
    group: Option<RequestGroup>,
    load_task: Option<Task<Loaded>>,
    pub(crate) loaded: bool,
    pub(crate) logo: Option<Vec<u8>>,
    pub(crate) logo_problem: bool,
    pub(crate) hidden: bool,
    viewer: ViewerState,
    pick_rx: Option<Receiver<Vec<std::path::PathBuf>>>,
    read_rx: Option<Receiver<Option<Vec<u8>>>>,
    upload_task: Option<Task<()>>,
    pending_logo: Option<Vec<u8>>,
    pub(crate) footer: String,
    pub(crate) overrides: BTreeMap<DocType, String>,
    pub(crate) numbers: Vec<(DocType, String, String)>,
    loaded_numbers: Vec<DocumentNumber>,
    save_task: Option<Task<()>>,
    pub(crate) numbering: bool,
    pub(crate) notice: Option<&'static str>,
    pub(crate) error: Option<ApiError>,
}

impl CompanyInfoScreen {
    /// Company Information of the open company.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, tenant: Option<(TenantId, String)>) -> CompanyInfoScreen {
        let group = link.group().and_then(Result::ok);
        let hidden = tenant
            .as_ref()
            .is_some_and(|(id, _)| logo_hidden(&link, *id));
        let numbering = link.is_demo() || link.capabilities().has(Capabilities::DOCUMENT_NUMBERS);
        let mut s = CompanyInfoScreen {
            link,
            tenant,
            group,
            load_task: None,
            loaded: false,
            logo: None,
            logo_problem: false,
            hidden,
            viewer: ViewerState::default(),
            pick_rx: None,
            read_rx: None,
            upload_task: None,
            pending_logo: None,
            footer: String::new(),
            overrides: BTreeMap::new(),
            numbers: Vec::new(),
            loaded_numbers: Vec::new(),
            save_task: None,
            numbering,
            notice: None,
            error: None,
        };
        s.load();
        s.poll();
        s
    }

    fn load(&mut self) {
        if let Some(d) = self.link.demo_state() {
            let l = Loaded {
                logo: crate::company::demo::printing::logo(&d),
                footers: crate::company::demo::printing::footers(&d),
                numbers: crate::company::demo::printing::numbers(&d),
            };
            drop(d);
            self.load_task = Some(Task::ready(l));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.loaded = true;
            return;
        };
        let caps = self.link.capabilities();
        let (logo_cap, numbers_cap) = (
            caps.has(Capabilities::COMPANY_LOGO),
            caps.has(Capabilities::DOCUMENT_NUMBERS),
        );
        if let Ok(p) = group.read(&session, move |s| async move {
            Ok(Loaded {
                logo: if logo_cap {
                    s.printing().logo().await?
                } else {
                    None
                },
                footers: s.printing().footers().await?,
                numbers: if numbers_cap {
                    s.printing().document_numbers().await?
                } else {
                    Vec::new()
                },
            })
        }) {
            self.load_task = Some(Task::Pending(p));
        }
    }

    fn show_logo(&mut self, bytes: Option<Vec<u8>>) {
        self.logo_problem = bytes
            .as_deref()
            .is_some_and(|b| cl_print::Logo::from_bytes(b).is_err());
        match &bytes {
            Some(b) => self.viewer.open(DocSource {
                name: "logo".into(),
                bytes: Arc::from(b.clone().into_boxed_slice()),
            }),
            None => self.viewer.clear(),
        }
        self.logo = bytes;
    }

    fn set_hidden(&mut self, on: bool) {
        self.hidden = on;
        if let Some((id, name)) = &self.tenant
            && let Err(e) = self.link.store().update(|l| {
                l.upsert(*id, name).prefs.hide_logo = on;
            })
        {
            tracing::warn!("could not save the logo choice: {e}");
        }
    }

    /// A chosen file: checked here (PNG or JPEG, 2 MB), then uploaded.
    pub(crate) fn upload(&mut self, bytes: Vec<u8>) {
        self.notice = None;
        self.error = None;
        if bytes.len() > cl_print::image::MAX_LOGO_BYTES
            || cl_print::Logo::from_bytes(&bytes).is_err()
        {
            self.notice = Some("company_info.logo.bad");
            return;
        }
        self.pending_logo = Some(bytes.clone());
        if let Some(mut d) = self.link.demo_state() {
            crate::company::demo::printing::set_logo(&mut d, Some(bytes));
            drop(d);
            self.upload_task = Some(Task::ready(()));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let label = OrphanLabel {
            what: "orphan.print_checks",
            key: None,
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            s.printing().upload_logo(&bytes).await
        }) {
            self.upload_task = Some(Task::Pending(p));
        }
    }

    /// Save the footers and every changed numbering row.
    pub(crate) fn save(&mut self) {
        self.notice = None;
        self.error = None;
        if self.footer.chars().count() > MAX_FOOTER_CHARS
            || self
                .overrides
                .values()
                .any(|o| o.chars().count() > MAX_FOOTER_CHARS)
        {
            self.notice = Some("company_info.footer.too_long");
            return;
        }
        let mut changes = Vec::new();
        for (kind, prefix, next) in &self.numbers {
            let Ok(n) = next.trim().parse::<u64>() else {
                self.notice = Some("company_info.numbers.bad");
                return;
            };
            if prefix.chars().any(char::is_whitespace)
                || prefix.chars().count() > cl_api::MAX_PREFIX_CHARS
                || !(1..=cl_api::MAX_NEXT_NUMBER).contains(&n)
            {
                self.notice = Some("company_info.numbers.bad");
                return;
            }
            let was = self.loaded_numbers.iter().find(|x| x.kind == *kind);
            if was.is_none_or(|w| w.prefix != *prefix || w.next_number != n) {
                changes.push((*kind, prefix.clone(), n));
            }
        }
        let footer = self.footer.clone();
        let overrides = self.overrides.clone();
        if let Some(mut d) = self.link.demo_state() {
            crate::company::demo::printing::set_footers(&mut d, &footer, &overrides);
            for (k, p, n) in &changes {
                crate::company::demo::printing::set_number(&mut d, *k, p, *n);
            }
            drop(d);
            self.save_task = Some(Task::ready(()));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let numbering = self.numbering;
        let label = OrphanLabel {
            what: "orphan.print_checks",
            key: None,
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            s.printing().set_footers(&footer, &overrides).await?;
            if numbering {
                for (k, p, n) in changes {
                    let u = DocumentNumberUpdate {
                        prefix: Some(p),
                        next_number: Some(n),
                    };
                    s.printing().set_document_number(k, &u).await?;
                }
            }
            Ok(())
        }) {
            self.save_task = Some(Task::Pending(p));
        }
    }

    fn poll(&mut self) {
        if let Some(r) = self.load_task.as_mut().and_then(Task::try_take) {
            self.load_task = None;
            self.loaded = true;
            match r {
                Ok(l) => {
                    self.footer = l.footers.footer_text.clone().unwrap_or_default();
                    self.overrides = DocType::ALL
                        .into_iter()
                        .map(|k| (k, l.footers.overrides.get(&k).cloned().unwrap_or_default()))
                        .collect();
                    self.numbers = l
                        .numbers
                        .iter()
                        .map(|n| (n.kind, n.prefix.clone(), n.next_number.to_string()))
                        .collect();
                    self.loaded_numbers = l.numbers;
                    self.show_logo(l.logo);
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(paths) = crate::widgets::file_picker::poll(&mut self.pick_rx)
            && let Some(path) = paths.into_iter().next()
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("cl-read-logo".into())
                .spawn(move || {
                    let r = std::fs::metadata(&path)
                        .ok()
                        .filter(|m| m.len() <= cl_print::image::MAX_LOGO_BYTES as u64)
                        .and_then(|_| std::fs::read(&path).ok());
                    let _ = tx.send(r);
                });
            if spawned.is_ok() {
                self.read_rx = Some(rx);
            }
        }
        if let Some(rx) = &self.read_rx
            && let Ok(r) = rx.try_recv()
        {
            self.read_rx = None;
            match r {
                Some(bytes) => self.upload(bytes),
                None => self.notice = Some("company_info.logo.bad"),
            }
        }
        if let Some(r) = self.upload_task.as_mut().and_then(Task::try_take) {
            self.upload_task = None;
            match r {
                Ok(()) => {
                    let b = self.pending_logo.take();
                    self.show_logo(b);
                    self.set_hidden(false);
                    self.notice = Some("company_info.logo.saved");
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = self.save_task.as_mut().and_then(Task::try_take) {
            self.save_task = None;
            match r {
                Ok(()) => {
                    self.notice = Some("company_info.saved");
                    self.load();
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        ui.label(
            RichText::new(t("cmd.company.information"))
                .size(22.0)
                .strong()
                .color(tokens.text_primary),
        );
        form::hint(ui, &tokens, &t("company_info.subtitle"));
        if let Some(k) = self.notice {
            form::hint(ui, &tokens, &t(k));
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        if !self.loaded {
            form::progress_line(ui, &tokens, &t("company_info.loading"));
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            self.logo_ui(ui, &tokens);
            ui.add_space(10.0);
            self.footer_ui(ui, &tokens);
            ui.add_space(10.0);
            self.numbers_ui(ui, &tokens);
            ui.add_space(10.0);
            if ui
                .add_enabled(
                    self.save_task.is_none(),
                    egui::Button::new(t("company_info.save")),
                )
                .clicked()
            {
                self.save();
            }
        });
    }

    fn logo_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let mut act = None;
        kit::panel_card(ui, tokens, &t("company_info.logo"), |ui| {
            form::hint(ui, tokens, &t("company_info.logo.hint"));
            if self.logo_problem {
                form::inline_warning(ui, tokens, &t("print.logo_notice"));
            }
            if self.logo.is_some() && self.hidden {
                form::hint(ui, tokens, &t("company_info.logo.removed"));
            } else if self.logo.is_some() {
                ui.allocate_ui(egui::vec2(ui.available_width().min(420.0), 160.0), |ui| {
                    DocViewer::show(ui, &mut self.viewer, tokens);
                });
            } else {
                form::hint(ui, tokens, &t("company_info.logo.none"));
            }
            ui.horizontal(|ui| {
                if ui.button(t("company_info.logo.browse")).clicked() {
                    act = Some("browse");
                }
                if self.logo.is_some()
                    && !self.hidden
                    && ui.button(t("company_info.logo.remove")).clicked()
                {
                    act = Some("remove");
                }
            });
        });
        match act {
            Some("browse") => {
                self.pick_rx = Some(crate::widgets::file_picker::pick_files(
                    ui.ctx(),
                    crate::widgets::file_picker::PickUse::Logo,
                    &[crate::widgets::file_picker::IMAGES],
                    false,
                ));
            }
            Some("remove") => {
                self.set_hidden(true);
                self.notice = Some("company_info.logo.removed");
            }
            _ => {}
        }
    }

    fn footer_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        kit::panel_card(ui, tokens, &t("company_info.footer"), |ui| {
            form::hint(ui, tokens, &t("company_info.footer.hint"));
            let l = ui.label(t("company_info.footer.company"));
            ui.add(
                egui::TextEdit::multiline(&mut self.footer)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(l.id);
            for k in DocType::ALL {
                let text = self.overrides.entry(k).or_default();
                let l = ui.label(crate::i18n::t_with(
                    "company_info.footer.override",
                    &[("type", &t(type_key(k)))],
                ));
                ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY))
                    .labelled_by(l.id);
            }
        });
    }

    fn numbers_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        kit::panel_card(ui, tokens, &t("company_info.numbers"), |ui| {
            if !self.numbering {
                form::hint(ui, tokens, &t("company_info.numbers.unavailable"));
                return;
            }
            form::hint(ui, tokens, &t("company_info.numbers.hint"));
            egui::Grid::new("company_info/numbers").show(ui, |ui| {
                ui.label(RichText::new(t("company_info.numbers.type")).strong());
                ui.label(RichText::new(t("company_info.numbers.prefix")).strong());
                ui.label(RichText::new(t("company_info.numbers.next")).strong());
                ui.label(RichText::new(t("company_info.numbers.shows")).strong());
                ui.end_row();
                for (k, prefix, next) in &mut self.numbers {
                    let name = t(type_key(*k));
                    let l = ui.label(&name);
                    ui.add(egui::TextEdit::singleline(prefix).desired_width(80.0))
                        .labelled_by(l.id)
                        .on_hover_text(crate::i18n::t_with(
                            "company_info.numbers.prefix_of",
                            &[("type", &name)],
                        ));
                    ui.add(egui::TextEdit::singleline(next).desired_width(100.0))
                        .labelled_by(l.id)
                        .on_hover_text(crate::i18n::t_with(
                            "company_info.numbers.next_of",
                            &[("type", &name)],
                        ));
                    ui.label(cl_model::numbering::format_number(
                        prefix,
                        next.trim().parse().unwrap_or(0),
                    ));
                    ui.end_row();
                }
            });
        });
    }

    fn state_json(&self) -> Value {
        json!({
            "screen": "company_info",
            "loaded": self.loaded,
            "logo": self.logo.is_some(),
            "logo_hidden": self.hidden,
            "logo_problem": self.logo_problem,
            "footer_set": !self.footer.trim().is_empty(),
            "overrides": self.overrides.values().filter(|v| !v.trim().is_empty()).count(),
            "numbers": self.numbers.iter().map(|(k, p, n)| json!([k.as_wire(), p, n])).collect::<Vec<_>>(),
            "numbering": self.numbering,
            "notice": self.notice,
            "error": self.error.is_some(),
        })
    }
}

impl Screen for CompanyInfoScreen {
    fn title(&self) -> String {
        t("cmd.company.information")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        self.draw(ui);
        let busy = self.load_task.is_some()
            || self.upload_task.is_some()
            || self.save_task.is_some()
            || self.pick_rx.is_some()
            || self.read_rx.is_some();
        if busy {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            KIND => true,
            "form.save_close" | "form.save_new" => {
                self.save();
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
mod tests {
    use super::*;

    /// A small PNG made here.
    fn png() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(4, 2, image::Rgb([10, 90, 60]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png)
            .expect("png");
        out.into_inner()
    }

    fn screen() -> (tempfile::TempDir, CompanyInfoScreen) {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let tenant = link
            .demo_state()
            .and_then(|d| d.companies.first().map(|c| (c.tenant_id, c.name.clone())));
        let s = CompanyInfoScreen::new(link, tenant);
        (dir, s)
    }

    #[test]
    fn logo_footers_and_numbers_round_trip_on_the_demo() {
        let (_d, mut s) = screen();
        assert!(s.loaded);
        assert_eq!(s.numbers.len(), 4);
        assert!(s.logo.is_none());
        s.upload(b"GIF89a not a logo".to_vec());
        assert_eq!(s.notice, Some("company_info.logo.bad"));
        s.upload(png());
        s.poll();
        assert_eq!(s.logo, Some(png()));
        assert!(!s.hidden);
        s.footer = "Thank you!".into();
        s.overrides.insert(DocType::Invoice, "Net 30".into());
        s.numbers[1].1 = "SR-".into();
        s.numbers[1].2 = "2001".into();
        s.save();
        s.poll();
        s.poll();
        assert_eq!(s.notice, Some("company_info.saved"));
        let d = s.link.demo_state().expect("demo");
        assert_eq!(
            crate::company::demo::printing::footers(&d).resolved_for(DocType::Invoice),
            "Net 30"
        );
        assert_eq!(
            crate::company::demo::printing::numbers(&d)[1].next,
            "SR-2001"
        );
        drop(d);
        s.numbers[0].1 = "A B".into();
        s.save();
        assert_eq!(s.notice, Some("company_info.numbers.bad"));
    }

    #[test]
    fn remove_hides_the_logo_for_this_company() {
        let (_d, mut s) = screen();
        s.upload(png());
        s.poll();
        s.set_hidden(true);
        let tenant = s.tenant.as_ref().map(|(t, _)| *t).expect("tenant");
        assert!(logo_hidden(&s.link, tenant));
        s.upload(png());
        s.poll();
        assert!(!logo_hidden(&s.link, tenant), "a new upload shows it again");
        assert!(!s.state().is_null());
    }
}
