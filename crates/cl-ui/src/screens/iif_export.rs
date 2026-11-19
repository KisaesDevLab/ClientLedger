//! Export IIF File (P32 D5, Q-126, Q-P32c, Q-P32j): lists and journal
//! entries for QuickBooks Desktop. The purpose line and the backup advice
//! are the only places the window names QuickBooks Desktop (migration
//! text, non-negotiable 5). A stop lists each transaction with its reason
//! and an Open button; a finished file shows what it can't carry.

#![allow(clippy::doc_markdown, reason = "QuickBooks is a product name")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use cl_company::Progress;
use cl_company::iif::fetch::{IifOutcome, IifRequest};
use cl_company::iif::{IifError, Limitation, Options, Stop, StopReason};
use cl_core::CalendarDate;
use cl_model::CommandId;
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink, Task, spawn_lifecycle};
use crate::i18n::{count_text, t, t_with};
use crate::screens::company::form::{self, DateInput};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The window kind.
pub const KIND: &str = "file.export_iif";

/// The window for `kind`, if it is this one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(cl_core::ids::TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND).then(|| Box::new(IifScreen::new(Arc::clone(link), open, today)) as _)
}

/// The IIF window.
pub struct IifScreen {
    link: Arc<CompanyLink>,
    group: Option<cl_api::RequestGroup>,
    today: CalendarDate,
    name: String,
    all_dates: bool,
    from: DateInput,
    to: DateInput,
    lists: bool,
    transactions: bool,
    save_rx: Option<Receiver<Option<PathBuf>>>,
    task: Option<Task<Result<IifOutcome, IifError>>>,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<Option<Progress>>>,
    outcome: Option<IifOutcome>,
    error: Option<IifError>,
}

impl IifScreen {
    /// A window over the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        open: Option<(cl_core::ids::TenantId, String)>,
        today: CalendarDate,
    ) -> IifScreen {
        let group = link.group().and_then(Result::ok);
        IifScreen {
            group,
            today,
            name: open.map_or_else(|| "Company".to_string(), |(_, n)| n),
            all_dates: true,
            from: DateInput::new(today.start_of_year()),
            to: DateInput::new(today),
            lists: true,
            transactions: true,
            save_rx: None,
            task: None,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Mutex::new(None)),
            outcome: None,
            error: None,
            link,
        }
    }

    /// The user sees every account (Q-P32i).
    fn sees_hidden(&self) -> bool {
        !self.link.rights().hides_sensitive
    }

    fn file_name(&self) -> String {
        cl_company::transfer::suggested_file_name(&self.name, self.today).replace(".clxfer", ".iif")
    }

    fn request(&self, dest: PathBuf) -> IifRequest {
        IifRequest {
            dest,
            start: (!self.all_dates).then_some(self.from.value).flatten(),
            end: (!self.all_dates).then_some(self.to.value).flatten(),
            options: Options {
                include_lists: self.lists,
                include_transactions: self.transactions,
            },
            sees_hidden: self.sees_hidden(),
        }
    }

    fn save(&mut self, ctx: Option<&egui::Context>) {
        self.error = None;
        self.outcome = None;
        if !self.sees_hidden() {
            self.error = Some(IifError::HiddenAccounts);
            return;
        }
        match ctx {
            Some(ctx) => {
                self.save_rx = Some(crate::widgets::file_picker::save_file_as(
                    ctx,
                    crate::widgets::file_picker::PickUse::SaveIif,
                    Some(&self.link.backup_dir),
                    &self.file_name(),
                    ("picker.filter.iif", "iif"),
                ));
            }
            None => self.start(self.link.backup_dir.join(self.file_name())),
        }
    }

    fn start(&mut self, path: PathBuf) {
        let dest = if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("iif"))
        {
            path
        } else {
            path.with_extension("iif")
        };
        let req = self.request(dest);
        self.cancel.store(false, Ordering::SeqCst);
        let demo = self
            .link
            .demo_state()
            .map(|mut d| crate::company::demo::transfer::iif_export(&mut d, &req));
        if let Some(r) = demo {
            self.task = Some(Task::ready(r));
            self.poll();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.error = Some(IifError::Api(cl_api::ApiError::NoCompanyOpen));
            return;
        };
        let slot = Arc::clone(&self.progress);
        let sink: cl_company::ProgressSink = Arc::new(move |p| {
            if let Ok(mut s) = slot.lock() {
                *s = Some(p);
            }
        });
        let cancel = Arc::clone(&self.cancel);
        self.task = spawn_lifecycle(&self.link, group, move || async move {
            cl_company::iif::fetch::export_iif(&session, &req, &sink, &cancel).await
        })
        .ok();
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.save_rx {
            match rx.try_recv() {
                Ok(Some(p)) => {
                    self.save_rx = None;
                    self.start(p);
                }
                Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => self.save_rx = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(task) = self.task.as_mut()
            && let Some(r) = task.poll_result()
        {
            self.task = None;
            match r {
                Ok(o) => {
                    tracing::info!(transactions = o.counts.transactions, "iif file saved");
                    self.outcome = Some(o);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "iif export stopped");
                    self.error = Some(e);
                }
            }
        }
    }

    fn open_stop(&self, s: &Stop) {
        let form =
            crate::screens::reports::drill::form_of(&s.txn_type).or(match s.txn_type.as_str() {
                "expense" => Some(crate::screens::write_check::KIND),
                _ => None,
            });
        if let (Some(form), Ok(id)) = (form, cl_core::ids::TransactionId::parse(&s.txn_id)) {
            self.link
                .push(CompanyEvent::OpenRecord(CommandId::new(form), id));
        }
    }

    fn error_key(&self) -> Option<&'static str> {
        Some(match self.error.as_ref()? {
            IifError::Stopped(_) => "iif.stopped",
            IifError::HiddenAccounts => "iif.hidden",
            IifError::BooksChanged => "iif.changed",
            IifError::Duplicate => "iif.duplicate",
            IifError::Inconsistent { .. } => "iif.inconsistent",
            IifError::Cancelled => "iif.cancelled",
            IifError::Io(_) | IifError::Api(_) => "iif.failed",
        })
    }
}

/// Plain words for a stop.
fn reason_text(r: &StopReason) -> String {
    match r {
        StopReason::NotFooting { off } => t_with(
            "iif.stop.not_footing",
            &[(
                "off",
                &cl_core::report::display_amount(*off, cl_core::MoneyStyle::default()),
            )],
        ),
        StopReason::ArWithoutClient => t("iif.stop.ar"),
        StopReason::ApWithoutVendor => t("iif.stop.ap"),
    }
}

/// Plain words for a limitation.
#[must_use]
pub fn limitation_text(l: &Limitation) -> String {
    let n = |k: &str, v: u64| t_with(k, &[("n", &count_text(v))]);
    match l {
        Limitation::FallbackAccountTypes(v) => n("iif.limit.fallback", *v),
        Limitation::ItemsLeftOut(v) => n("iif.limit.items", *v),
        Limitation::InventoryAsNonInventory(v) => n("iif.limit.inventory", *v),
        Limitation::Unmappable(v) => n("iif.limit.unmappable", *v),
        Limitation::Renamed(v) => n("iif.limit.renamed", *v),
        Limitation::SeveralArApLines(v) => n("iif.limit.arap", *v),
        Limitation::Unconfirmed => t("iif.limit.unconfirmed"),
    }
}

impl Screen for IifScreen {
    fn title(&self) -> String {
        t("cmd.file.export_iif")
    }

    fn can_save(&self) -> bool {
        false
    }

    #[allow(clippy::too_many_lines, reason = "one dialog, top to bottom")]
    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        if self.task.is_some() || self.save_rx.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        let tokens = Tokens::of(ui.ctx());
        let mut save = false;
        let mut open: Option<Stop> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                form::heading(ui, &tokens, &t("iif.title"));
                form::hint(ui, &tokens, &t("iif.purpose"));
                form::inline_warning(ui, &tokens, &t("iif.backup_advice"));
                form::inline_warning(ui, &tokens, &t("iif.plaintext"));
                if !self.sees_hidden() {
                    form::inline_warning(ui, &tokens, &t("iif.hidden"));
                }
                ui.add_space(6.0);
                let running = self.task.is_some();
                form::card(ui, &tokens, |ui| {
                    ui.add_enabled_ui(!running, |ui| {
                        ui.label(RichText::new(t("iif.range")).strong());
                        ui.checkbox(&mut self.all_dates, t("iif.range.all"));
                        if !self.all_dates {
                            form::date_field(ui, &t("iif.range.from"), &mut self.from, self.today);
                            form::date_field(ui, &t("iif.range.to"), &mut self.to, self.today);
                        }
                        ui.checkbox(&mut self.lists, t("iif.lists"));
                        ui.checkbox(&mut self.transactions, t("iif.transactions"));
                    });
                    ui.add_space(6.0);
                    if running {
                        let text = match self.progress.lock().ok().and_then(|p| p.clone()) {
                            Some(Progress::Working {
                                percent: Some(p), ..
                            }) => t_with("iif.working.percent", &[("p", &p.to_string())]),
                            _ => t("iif.working"),
                        };
                        form::progress_line(ui, &tokens, &text);
                        if ui.button(t("iif.cancel")).clicked() {
                            self.cancel.store(true, Ordering::SeqCst);
                        }
                    } else if ui
                        .add_enabled(
                            self.sees_hidden() && (self.lists || self.transactions),
                            egui::Button::new(RichText::new(t("iif.save")).strong()),
                        )
                        .clicked()
                    {
                        save = true;
                    }
                });
                if let Some(e) = &self.error {
                    ui.add_space(6.0);
                    form::card(ui, &tokens, |ui| {
                        let key = self.error_key().unwrap_or("iif.failed");
                        form::inline_warning(ui, &tokens, &t(key));
                        if let IifError::Stopped(stops) = e {
                            egui::Grid::new("iif_stops")
                                .num_columns(5)
                                .striped(true)
                                .show(ui, |ui| {
                                    for h in [
                                        "iif.stop.date",
                                        "iif.stop.type",
                                        "iif.stop.number",
                                        "iif.stop.reason",
                                    ] {
                                        ui.label(RichText::new(t(h)).color(tokens.text_secondary));
                                    }
                                    ui.label("");
                                    ui.end_row();
                                    for s in stops {
                                        ui.label(crate::shell::us_date(s.date));
                                        ui.label(&s.txn_type);
                                        ui.label(s.number.clone().unwrap_or_default());
                                        ui.label(reason_text(&s.reason));
                                        if ui.button(t("iif.stop.open")).clicked() {
                                            open = Some(s.clone());
                                        }
                                        ui.end_row();
                                    }
                                });
                            form::hint(ui, &tokens, &t("iif.stop.fix"));
                        }
                    });
                }
                if let Some(o) = &self.outcome {
                    ui.add_space(6.0);
                    form::card(ui, &tokens, |ui| {
                        ui.label(
                            RichText::new(t_with(
                                "iif.done",
                                &[("path", &o.path.display().to_string())],
                            ))
                            .strong(),
                        );
                        let c = o.counts;
                        ui.label(t_with(
                            "iif.done.counts",
                            &[
                                ("accounts", &count_text(c.accounts)),
                                ("clients", &count_text(c.clients)),
                                ("vendors", &count_text(c.vendors)),
                                ("items", &count_text(c.items)),
                                ("transactions", &count_text(c.transactions)),
                            ],
                        ));
                        ui.add_space(4.0);
                        ui.label(RichText::new(t("iif.limits")).strong());
                        ui.label(t("iif.limit.always"));
                        for l in &o.limitations {
                            ui.label(format!("\u{2022} {}", limitation_text(l)));
                        }
                    });
                }
            });
        if save {
            let ctx = ui.ctx().clone();
            self.save(Some(&ctx));
        }
        if let Some(s) = open {
            self.open_stop(&s);
        }
    }

    fn command(&mut self, id: CommandId) -> bool {
        if id.as_str() != "dev.iif_demo_unbalanced" {
            return false;
        }
        let Some(mut d) = self.link.demo_state() else {
            return false;
        };
        d.transfer.iif_unbalanced = true;
        true
    }

    fn state(&self) -> Value {
        json!({
            "screen": "iif_export",
            "running": self.task.is_some(),
            "done": self.outcome.is_some(),
            "error": self.error_key(),
            "stops": match &self.error { Some(IifError::Stopped(s)) => s.len(), _ => 0 },
            "limitations": self.outcome.as_ref().map_or(0, |o| o.limitations.len()),
            "transactions": self.outcome.as_ref().map(|o| o.counts.transactions),
            "sees_hidden": self.sees_hidden(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> (tempfile::TempDir, IifScreen) {
        let dir = tempfile::tempdir().expect("tmp");
        let mut link = CompanyLink::demo(dir.path());
        link.backup_dir = dir.path().join("out");
        let today = CalendarDate::from_ymd(2026, 11, 4).expect("d");
        (dir, IifScreen::new(Arc::new(link), None, today))
    }

    #[test]
    fn a_stop_writes_nothing_and_a_good_file_lists_its_limits() {
        let (_d, mut s) = screen();
        std::fs::create_dir_all(&s.link.backup_dir).expect("dir");
        assert!(s.command(CommandId::new("dev.iif_demo_unbalanced")));
        s.save(None);
        assert_eq!(s.state()["error"], "iif.stopped");
        assert_eq!(s.state()["stops"], 1);
        assert!(!s.link.backup_dir.join(s.file_name()).exists());
        s.save(None);
        assert_eq!(s.state()["done"], true);
        assert_eq!(s.state()["transactions"], 2);
        assert!(s.link.backup_dir.join(s.file_name()).is_file());
        let o = s.outcome.as_ref().expect("outcome");
        assert!(o.limitations.contains(&Limitation::Unconfirmed));
        for l in &o.limitations {
            assert!(!limitation_text(l).starts_with("iif."), "{l:?}");
        }
        for k in [
            "iif.purpose",
            "iif.backup_advice",
            "iif.plaintext",
            "iif.stop.not_footing",
            "picker.title.save_iif",
        ] {
            assert!(crate::i18n::has(k), "{k}");
        }
    }
}
