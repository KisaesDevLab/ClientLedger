//! Drawing Import Payroll: the heading, the step bar, each step, the
//! dialogs (overlap, closing-date password) and History. Every input has
//! a visible label it is `labelled_by` (AccessKit, control channel).

use cl_api::payroll::{PayrollRefusal, SessionStatus};
use cl_core::MoneyStyle;
use cl_core::ids::{AccountId, TagId};
use cl_model::CommandId;
use cl_model::payroll_import::{FileMode, LayoutChoice, MAPPABLE_FIELDS, Provider, Step};
use eframe::egui::{self, Margin, RichText, Ui};

use super::{PayrollImportScreen, Trouble, View, show_date};
use crate::company::CompanyEvent;
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts};
use crate::theme::Tokens;

fn shortcut_hint(id: &str) -> String {
    cl_model::commands::find(id)
        .and_then(|c| c.shortcut)
        .map(crate::input::shortcut_text)
        .unwrap_or_default()
}

fn heading(ui: &mut Ui, tokens: &Tokens, key: &str) {
    ui.add(egui::Label::new(
        RichText::new(t(key))
            .size(16.0)
            .strong()
            .color(tokens.text_primary),
    ));
}

fn note(ui: &mut Ui, tokens: &Tokens, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(tokens.text_secondary)).wrap());
}

fn warn(ui: &mut Ui, tokens: &Tokens, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(tokens.warn_text)).wrap());
}

fn step_name(s: Step) -> String {
    t(&format!("payroll.step.{}", s.key()))
}

fn account_name(chart: &[cl_api::Account], id: Option<AccountId>) -> String {
    id.and_then(|id| chart.iter().find(|a| a.id == id))
        .map_or_else(
            || t("payroll.accounts.choose"),
            |a| {
                a.fields.account_number.as_ref().map_or_else(
                    || a.fields.name.clone(),
                    |n| format!("{n} · {}", a.fields.name),
                )
            },
        )
}

fn tag_name(tags: &[cl_api::Tag], id: Option<TagId>) -> String {
    id.and_then(|id| tags.iter().find(|x| x.id == id))
        .map_or_else(|| t("payroll.map.no_tag"), |x| x.fields.name.clone())
}

/// An account picker; returns the chosen account.
fn account_combo(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    chart: &[cl_api::Account],
    current: Option<AccountId>,
) -> Option<AccountId> {
    let mut chosen = None;
    let l = ui.label(label);
    let shown = chart.iter().filter(|a| a.is_active).count();
    egui::ComboBox::from_id_salt(id)
        .selected_text(account_name(chart, current))
        .width(260.0)
        .height(list_height(ui, shown))
        .show_ui(ui, |ui| {
            for a in chart.iter().filter(|a| a.is_active) {
                if ui
                    .selectable_label(Some(a.id) == current, account_name(chart, Some(a.id)))
                    .clicked()
                {
                    chosen = Some(a.id);
                }
            }
        })
        .response
        .labelled_by(l.id);
    chosen
}

/// A tag picker; `Some(choice)` when the owner picked (None = no tag).
#[allow(clippy::option_option)] // outer: picked or not; inner: the tag or no tag
fn tag_combo(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    tags: &[cl_api::Tag],
    current: Option<TagId>,
    show_label: bool,
) -> Option<Option<TagId>> {
    let mut chosen = None;
    let l = show_label.then(|| ui.label(label));
    let r = egui::ComboBox::from_id_salt(id)
        .width(150.0)
        .selected_text(tag_name(tags, current))
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(current.is_none(), t("payroll.map.no_tag"))
                .clicked()
            {
                chosen = Some(None);
            }
            for x in tags {
                if ui
                    .selectable_label(Some(x.id) == current, &x.fields.name)
                    .clicked()
                {
                    chosen = Some(Some(x.id));
                }
            }
        })
        .response;
    match l {
        Some(l) => {
            r.labelled_by(l.id);
        }
        // No visible label (a grid column says it): the name is the
        // widget's own, for AccessKit and the control channel.
        None => r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, label)),
    }
    chosen
}

impl PayrollImportScreen {
    pub(super) fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        egui::Frame::new()
            .fill(tokens.surface_window)
            .inner_margin(Margin::symmetric(20, 12))
            .show(ui, |ui| {
                let mut frame = FixedFrame::begin(ui, "payroll_import", FrameOpts::default());
                frame.top(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.add(egui::Label::new(
                            RichText::new(t("cmd.payroll.import"))
                                .size(22.0)
                                .strong()
                                .color(tokens.text_primary),
                        ));
                        ui.add_space(12.0);
                        if ui
                            .selectable_label(self.view == View::Import, t("payroll.tab.import"))
                            .clicked()
                        {
                            self.show_import();
                        }
                        if ui
                            .selectable_label(self.view == View::History, t("payroll.tab.history"))
                            .clicked()
                        {
                            self.show_history();
                        }
                    });
                    if self.view == View::Import {
                        note(ui, &tokens, &t("payroll.subtitle"));
                        ui.horizontal_wrapped(|ui| {
                            for (i, s) in Step::ALL.iter().enumerate() {
                                let text = format!("{}. {}", i + 1, step_name(*s));
                                let rich = if *s == self.step {
                                    RichText::new(text).strong().color(tokens.accent)
                                } else {
                                    RichText::new(text).color(tokens.text_secondary)
                                };
                                ui.add(egui::Label::new(rich));
                            }
                        });
                    }
                });
                frame.body(Body::Scroll, |ui| {
                    ui.set_width(ui.available_width());
                    self.trouble_ui(ui, &tokens, style);
                    if !self.allowed() || !self.supported() {
                        return;
                    }
                    match self.view {
                        View::History => self.history_ui(ui, &tokens),
                        View::Import => match self.step {
                            Step::Choose => self.choose_ui(ui, &tokens),
                            Step::Map => self.map_ui(ui, &tokens),
                            Step::Accounts => self.accounts_ui(ui, &tokens),
                            Step::Check => self.check_ui(ui, &tokens),
                            Step::Preview => self.preview_ui(ui, &tokens, style),
                            Step::Done => self.done_ui(ui, &tokens),
                        },
                    }
                    self.dialogs_ui(ui, &tokens);
                });
                frame.bottom(|ui| {
                    ui.add_space(4.0);
                    // Next to Post, never below the last entry (QA-3).
                    if self.view == View::Import {
                        self.password_ui(ui, &tokens);
                        self.backup_ui(ui, &tokens);
                    }
                    self.buttons_ui(ui);
                });
                let _ = frame.end();
            });
    }

    fn trouble_ui(&mut self, ui: &mut Ui, tokens: &Tokens, style: MoneyStyle) {
        let Some(tr) = self.trouble.clone() else {
            return;
        };
        // The prompt says these beside its field (QA-7).
        if self.asking_password.is_some()
            && matches!(tr, Trouble::PasswordWrong | Trouble::PasswordWait)
        {
            return;
        }
        let text = match &tr {
            Trouble::Refused(r) => match r {
                PayrollRefusal::DuplicateFile { .. } | PayrollRefusal::DuplicateRun { .. } => {
                    // QA-12: the refusal's date, else the earlier import's.
                    match self.prior_posted_at().as_deref().and_then(posted_day) {
                        Some(d) => t_with("payroll.refused.duplicate", &[("date", &show_date(d))]),
                        None => t("payroll.refused.duplicate_undated"),
                    }
                }
                PayrollRefusal::AlreadyPosted => t("payroll.refused.already_posted"),
                PayrollRefusal::NotReady => t("payroll.refused.not_ready"),
                PayrollRefusal::PreviewChanged => t("payroll.refused.preview_changed"),
                PayrollRefusal::NotPosted => t("payroll.refused.not_posted"),
                PayrollRefusal::Unbalanced { date, difference } => t_with(
                    "payroll.refused.unbalanced",
                    &[
                        ("date", &date.map(show_date).unwrap_or_default()),
                        (
                            "amount",
                            &difference.map(|d| d.format(style)).unwrap_or_default(),
                        ),
                    ],
                ),
                PayrollRefusal::Unmapped { line_types } => t_with(
                    "payroll.refused.unmapped",
                    &[(
                        "lines",
                        &line_types
                            .iter()
                            .map(|l| {
                                let k = format!("payroll.line.{l}");
                                if crate::i18n::has(&k) {
                                    t(&k)
                                } else {
                                    l.clone()
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(", "),
                    )],
                ),
                PayrollRefusal::AmountInvalid => t("payroll.refused.amounts"),
                PayrollRefusal::DateInvalid { rows } => undated_rows_text(rows),
                PayrollRefusal::DiscardUnsupported => t("payroll.refused.other"),
            },
            Trouble::Unknown => t("payroll.unknown"),
            Trouble::PasswordWrong => t("payroll.password_wrong"),
            Trouble::PasswordWait => t("payroll.password_wait"),
            Trouble::Restricted => t("payroll.restricted"),
            Trouble::Unsupported => t("payroll.unsupported"),
            Trouble::EngineDown => t("payroll.engine_down"),
            Trouble::NotSent => t("payroll.engine_down_not_sent"),
            Trouble::EngineBack { post: true } => t("payroll.engine_back_post"),
            Trouble::EngineBack { post: false } => t("payroll.engine_back"),
            Trouble::FileUnreadable => t("payroll.file.unreadable"),
            Trouble::Discarded => t("payroll.refused.discarded"),
            Trouble::Other => t("payroll.refused.other"),
        };
        warn(ui, tokens, &text);
        if tr == Trouble::Unknown {
            ui.horizontal(|ui| {
                if ui.button(t("payroll.check_status")).clicked() {
                    self.check_status();
                }
                // While a backup is made or after it failed, the backup's
                // own line offers Try again (one button, Q-P30h).
                if self.backup_failed.is_none()
                    && !self.backing_up()
                    && ui.button(t("payroll.try_again")).clicked()
                {
                    self.try_again();
                }
            });
            if self.view == View::Import && self.post_unsettled {
                note(ui, tokens, &t("payroll.unknown_kept"));
            }
        }
        if matches!(
            tr,
            Trouble::Refused(
                PayrollRefusal::DuplicateFile { .. } | PayrollRefusal::DuplicateRun { .. }
            )
        ) && ui.button(t("payroll.done.open_history")).clicked()
        {
            self.show_history();
        }
        if let Some(posted) = self.status_answer {
            note(
                ui,
                tokens,
                &t(if posted {
                    "payroll.status.posted"
                } else {
                    "payroll.status.not_posted"
                }),
            );
        }
        ui.add_space(8.0);
    }

    fn choose_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let l = ui.label(t("payroll.provider"));
        let mut provider = self.provider;
        // QA-2: every service shows without scrolling (egui's default list
        // fits about six; Rippling and "Other" were out of reach).
        egui::ComboBox::from_id_salt("payroll.provider")
            .selected_text(t(&format!("payroll.provider.{}", provider.key())))
            .width(280.0)
            .height(list_height(ui, Provider::ALL.len()))
            .show_ui(ui, |ui| {
                for p in Provider::ALL {
                    ui.selectable_value(
                        &mut provider,
                        p,
                        t(&format!("payroll.provider.{}", p.key())),
                    );
                }
            })
            .response
            .labelled_by(l.id);
        if provider != self.provider {
            self.provider = provider;
            self.forget_choice();
        }
        if self.provider == Provider::Rippling {
            note(ui, tokens, &t("payroll.provider.rippling_note"));
        }
        ui.add_space(10.0);
        ui.label(t("payroll.file"));
        ui.horizontal(|ui| {
            if ui.button(t("payroll.file.browse")).clicked() {
                self.browse(ui.ctx());
            }
            let text = match &self.file {
                Some((_, b)) => t_with(
                    "payroll.file.chosen",
                    &[(
                        "size",
                        &t_with(
                            "payroll.file.kb",
                            &[("n", &b.len().div_ceil(1024).to_string())],
                        ),
                    )],
                ),
                None => t("payroll.file.none"),
            };
            ui.label(text);
        });
        note(ui, tokens, &t("payroll.file.hint"));
        if let Some(k) = self.file_error {
            warn(ui, tokens, &t(k));
        }
        ui.add_space(10.0);
        for (i, key) in [
            "payroll.period_start",
            "payroll.period_end",
            "payroll.check_date",
        ]
        .iter()
        .enumerate()
        {
            ui.horizontal(|ui| {
                let l = ui.label(t(key));
                ui.add(
                    egui::TextEdit::singleline(&mut self.period_text[i])
                        .hint_text(t("payroll.date.hint"))
                        .desired_width(120.0),
                )
                .labelled_by(l.id);
            });
        }
        note(ui, tokens, &t("payroll.period.help"));
        if let Some(p) = self.period_problem {
            warn(ui, tokens, &t(p.key()));
        }
        if self.upload_job_running() {
            note(ui, tokens, &t("payroll.uploading"));
        }
    }

    /// The early duplicate warning (QA-6): a run the engine knows was
    /// posted, or the same file chosen before (posted or not).
    fn duplicate_ui(&self, ui: &mut Ui, tokens: &Tokens) {
        if let Some(text) = self.duplicate_text() {
            warn(ui, tokens, &text);
        }
        // QA-11: the file reads as another service's.
        if self.service_mismatch() {
            let name = t(&format!("payroll.provider.{}", self.provider.key()));
            warn(
                ui,
                tokens,
                &t_with("payroll.service_mismatch", &[("service", &name)]),
            );
        }
    }
    /// The early duplicate warning's text, with the earlier post's local
    /// date when it is known (QA-12).
    pub(crate) fn duplicate_text(&self) -> Option<String> {
        let u = self.upload.as_ref()?;
        let date = self
            .prior_posted_at()
            .as_deref()
            .and_then(posted_day)
            .map(show_date);
        Some(match (u.is_duplicate_by_key, u.is_duplicate, date) {
            (true, _, Some(d)) => t_with("payroll.duplicate_posted_on", &[("date", &d)]),
            (true, _, None) => t("payroll.duplicate_posted"),
            (false, true, Some(d)) => t_with("payroll.duplicate_file_posted", &[("date", &d)]),
            (false, true, None) => t("payroll.duplicate_warning"),
            (false, false, _) => return None,
        })
    }

    fn upload_job_running(&self) -> bool {
        self.upload.is_none()
            && self.step == Step::Choose
            && !self.can_next()
            && self.file.is_some()
            && self.period().checked(self.today).is_ok()
    }

    fn forget_choice(&mut self) {
        self.layout_choice = None;
        self.department_tags.clear();
        self.departments.clear();
        self.file_departments = None;
        self.departments_truncated = false;
    }

    fn map_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        heading(ui, tokens, "payroll.map.heading");
        note(ui, tokens, &t("payroll.map.hint"));
        self.duplicate_ui(ui, tokens);

        match &self.layout_choice {
            Some(LayoutChoice::Saved(id)) => {
                let name = self.saved_name(*id);
                note(
                    ui,
                    tokens,
                    &t_with("payroll.map.saved_banner", &[("name", &name)]),
                );
            }
            Some(LayoutChoice::Builtin) => note(ui, tokens, &t("payroll.map.builtin_banner")),
            _ => {}
        }
        if !self.new_columns.is_empty() {
            // S4: column names only (never cell text).
            warn(
                ui,
                tokens,
                &t_with(
                    "payroll.map.new_columns",
                    &[("columns", &self.new_columns.join(", "))],
                ),
            );
        }
        let headers = self
            .upload
            .as_ref()
            .map(|u| u.headers.clone())
            .unwrap_or_default();
        // The column list shows every column of a typical file without
        // scrolling (egui's default fits about six), capped to the window.
        let list_height = column_list_height(ui, headers.len());
        egui::Grid::new("payroll.map.grid")
            .num_columns(3)
            .striped(true)
            .show(ui, |ui| {
                ui.strong(t("payroll.map.column"));
                ui.label("");
                ui.strong(t("payroll.map.sample"));
                ui.end_row();
                for field in MAPPABLE_FIELDS {
                    let l = ui.label(t(&format!("payroll.field.{field}")));
                    let current = self.layout.column(field).map(str::to_string);
                    let mut chosen: Option<Option<String>> = None;
                    egui::ComboBox::from_id_salt(("payroll.map", field))
                        .selected_text(
                            current
                                .clone()
                                .unwrap_or_else(|| t("payroll.map.no_column")),
                        )
                        .width(220.0)
                        .height(list_height)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(current.is_none(), t("payroll.map.no_column"))
                                .clicked()
                            {
                                chosen = Some(None);
                            }
                            for h in &headers {
                                if ui
                                    .selectable_label(current.as_deref() == Some(h.as_str()), h)
                                    .clicked()
                                {
                                    chosen = Some(Some(h.clone()));
                                }
                            }
                        })
                        .response
                        .labelled_by(l.id);
                    if let Some(c) = chosen {
                        self.set_column(field, c);
                    }
                    ui.label(self.sample(field).unwrap_or_default());
                    ui.end_row();
                }
            });
        let missing = self.missing_fields();
        if !missing.is_empty() {
            let names: Vec<String> = missing
                .iter()
                .map(|f| t(&format!("payroll.field.{f}")))
                .collect();
            warn(
                ui,
                tokens,
                &t_with("payroll.map.required", &[("fields", &names.join(", "))]),
            );
        }
        ui.checkbox(&mut self.save_layout, t("payroll.map.save_layout"));
        self.departments_ui(ui, tokens);
    }

    /// Department → tag (Map, from the sample rows; Check, every department
    /// in the file when the engine lists them, S3).
    fn departments_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.departments.is_empty() || self.layout.column("department").is_none() {
            return;
        }
        ui.add_space(10.0);
        heading(ui, tokens, "payroll.map.departments");
        note(ui, tokens, &t("payroll.map.department_hint"));
        if self.file_departments.is_none() && self.step == Step::Map {
            note(ui, tokens, &t("payroll.map.departments_more"));
        }
        if self.departments_truncated {
            warn(ui, tokens, &t("payroll.map.departments_truncated"));
        }
        for d in self.departments.clone() {
            ui.horizontal(|ui| {
                let current = self.department_tags.get(&d).copied().flatten();
                if let Some(c) = tag_combo(ui, ("payroll.dept", &d), &d, &self.tags, current, true)
                {
                    self.department_tags.insert(d.clone(), c);
                }
            });
        }
    }

    fn saved_name(&self, id: uuid::Uuid) -> String {
        self.templates_named(id).unwrap_or_default()
    }

    fn templates_named(&self, id: uuid::Uuid) -> Option<String> {
        self.templates_list()
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.name.clone())
    }

    fn accounts_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        heading(ui, tokens, "payroll.accounts.heading");
        note(ui, tokens, &t("payroll.accounts.hint"));
        self.duplicate_ui(ui, tokens);

        if let Some(LayoutChoice::Saved(id)) = &self.layout_choice
            && self.provider.is_generic()
            && !self.visited.contains(&Step::Map)
        {
            let name = self.saved_name(*id);
            ui.horizontal(|ui| {
                note(
                    ui,
                    tokens,
                    &t_with("payroll.map.saved_banner", &[("name", &name)]),
                );
                if ui.button(t("payroll.map.change")).clicked() {
                    self.match_again();
                }
            });
        }
        let chart = self.chart.clone();
        // Labels in one column, pickers lined up in the next.
        if self.provider.mode() == FileMode::Prebuilt {
            egui::Grid::new("payroll.accounts.desc")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    for i in 0..self.descriptions.len() {
                        let d = self.descriptions[i].clone();
                        if let Some(a) = account_combo(
                            ui,
                            ("payroll.desc", i),
                            &d.description,
                            &chart,
                            d.account,
                        ) {
                            self.descriptions[i].account = Some(a);
                        }
                        ui.end_row();
                    }
                });
        } else {
            let lines = self.needed_lines();
            egui::Grid::new("payroll.accounts.lines")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    for line in lines {
                        let label = if line == cl_model::payroll_import::NET_PAY_LINE {
                            t("payroll.accounts.net_pay")
                        } else {
                            t(&format!("payroll.line.{line}"))
                        };
                        let current = self.line_accounts.get(line).copied();
                        if let Some(a) =
                            account_combo(ui, ("payroll.acct", line), &label, &chart, current)
                        {
                            self.line_accounts.insert(line.to_string(), a);
                        }
                        ui.end_row();
                    }
                });
            if !self.net_pay_from_bank() {
                warn(ui, tokens, &t("payroll.accounts.not_bank"));
            }
        }
        if !self.accounts_complete() {
            note(ui, tokens, &t("payroll.accounts.missing"));
        }
    }

    fn check_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        heading(ui, tokens, "payroll.check.heading");
        let Some(v) = self.validation.clone() else {
            note(ui, tokens, &t("payroll.checking"));
            return;
        };
        let line = |m: &cl_api::payroll::ValidationMessage| {
            let fk = format!("payroll.field.{}", m.field);
            let field = if crate::i18n::has(&fk) {
                t(&fk)
            } else {
                t("payroll.field.unknown")
            };
            let pk = format!("payroll.problem.{}", m.code);
            let problem = if crate::i18n::has(&pk) {
                t(&pk)
            } else {
                t("payroll.problem.other")
            };
            match m.row_number {
                Some(r) => t_with(
                    "payroll.check.row",
                    &[
                        ("row", &r.to_string()),
                        ("field", &field),
                        ("problem", &problem),
                    ],
                ),
                None => t_with(
                    "payroll.check.file",
                    &[("field", &field), ("problem", &problem)],
                ),
            }
        };
        let errors: Vec<_> = v.messages.iter().filter(|m| m.is_error).collect();
        let notes = note_groups(&v.messages);
        if errors.is_empty() {
            let rows = usize::try_from(v.total_rows).unwrap_or(usize::MAX);
            if self.tags_still_needed() {
                // QA-14: never "nothing stops posting" while every line
                // still needs a tag; say where to add them.
                warn(ui, tokens, &t_count("payroll.check.ok_tags", rows));
            } else {
                let key = if notes.is_empty() {
                    "payroll.check.ok"
                } else {
                    "payroll.check.ok_notes"
                };
                note(ui, tokens, &t_count(key, rows));
            }
        } else {
            warn(ui, tokens, &t("payroll.check.errors"));
            for m in errors.iter().take(50) {
                warn(ui, tokens, &line(m));
            }
        }
        // QA-5: the engine's notes, one line per kind with a row count and
        // its reason; marked as not stopping the post. No cell text.
        if !notes.is_empty() {
            ui.add_space(6.0);
            note(ui, tokens, &t("payroll.check.warnings"));
            for (code, rows) in &notes {
                let reason = note_reason(code);
                let text = if *rows == 0 {
                    reason
                } else {
                    t_with(
                        "payroll.check.note_rows",
                        &[
                            ("reason", &reason),
                            ("rows", &t_count("payroll.rows", *rows)),
                        ],
                    )
                };
                ui.label(format!("\u{2022} {text}"));
            }
        }
        // S3: every department in the file (the Map step saw only the
        // first rows), tagged before the entries are made.
        if self.file_departments.is_some() {
            self.departments_ui(ui, tokens);
        }
    }

    fn preview_ui(&mut self, ui: &mut Ui, tokens: &Tokens, style: MoneyStyle) {
        heading(ui, tokens, "payroll.preview.heading");
        note(ui, tokens, &t("payroll.preview.hint"));
        // The refusal (with its date) says it already when Post was refused.
        if !matches!(self.trouble, Some(Trouble::Refused(_))) {
            self.duplicate_ui(ui, tokens);
        }
        let Some(p) = self.preview.clone() else {
            note(ui, tokens, &t("payroll.preview.loading"));
            return;
        };
        let chart = self.chart.clone();
        let tags = self.tags.clone();
        for (ei, e) in p.entries.iter().enumerate() {
            ui.add_space(8.0);
            ui.strong(t_with(
                "payroll.preview.entry",
                &[("date", &show_date(e.date))],
            ));
            egui::Grid::new(("payroll.preview", ei))
                .num_columns(6)
                .striped(true)
                .show(ui, |ui| {
                    for k in [
                        "payroll.preview.account",
                        "payroll.preview.line",
                        "payroll.preview.department",
                        "payroll.preview.tag",
                        "payroll.preview.debit",
                        "payroll.preview.credit",
                    ] {
                        if k.ends_with("debit") || k.ends_with("credit") {
                            amount_cell(ui, RichText::new(t(k)).strong());
                        } else {
                            ui.strong(t(k));
                        }
                    }
                    ui.end_row();
                    for (li, l) in e.lines.iter().enumerate() {
                        ui.label(
                            l.account_name
                                .clone()
                                .unwrap_or_else(|| account_name(&chart, l.account)),
                        );
                        ui.label(&l.description);
                        ui.label(l.department.clone().unwrap_or_default());
                        let plan_line = self
                            .plan
                            .lines
                            .iter()
                            .find(|x| x.entry == ei && x.line == li)
                            .cloned();
                        let current = plan_line.as_ref().and_then(|x| self.plan.final_tag(x));
                        let label = format!("{} {}", t("payroll.preview.tag"), l.description);
                        let picked = ui
                            .horizontal(|ui| {
                                tag_combo(
                                    ui,
                                    ("payroll.tag", ei, li),
                                    &label,
                                    &tags,
                                    current,
                                    false,
                                )
                            })
                            .inner;
                        if let Some(c) = picked {
                            self.plan.set(ei, li, c);
                            self.untagged = None;
                        }
                        for m in [l.debit, l.credit] {
                            let text = if m.is_zero() {
                                String::new()
                            } else {
                                m.format(style)
                            };
                            amount_cell(ui, RichText::new(text));
                        }
                        ui.end_row();
                    }
                    ui.strong(t("payroll.preview.totals"));
                    ui.label("");
                    ui.label("");
                    ui.label(if e.is_balanced {
                        t("payroll.preview.balanced")
                    } else {
                        String::new()
                    });
                    amount_cell(ui, RichText::new(e.total_debits.format(style)).strong());
                    amount_cell(ui, RichText::new(e.total_credits.format(style)).strong());
                    ui.end_row();
                });
            if !e.is_balanced {
                warn(
                    ui,
                    tokens,
                    &t_with(
                        "payroll.preview.unbalanced",
                        &[("amount", &e.difference.abs().format(style))],
                    ),
                );
            }
        }
        if let Some(u) = &self.untagged {
            warn(
                ui,
                tokens,
                &t_with(
                    "payroll.untagged",
                    &[
                        ("date", &show_date(u.date)),
                        ("line", &u.description),
                        ("n", &u.count.to_string()),
                    ],
                ),
            );
        }
    }

    fn done_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        heading(ui, tokens, "payroll.done.heading");
        let n = self.posted.as_ref().map_or(0, Vec::len);
        ui.label(t_count("payroll.done.entries", n));
        let w = self.expected_withdrawals();
        if w > 0 {
            ui.label(t_count("payroll.done.withdrawals", w));
        }
        note(ui, tokens, &t("payroll.done.discarded"));
        ui.horizontal(|ui| {
            // QA-9: the next pay run without closing the window (same
            // service; the accounts chosen stay).
            if ui.button(t("payroll.done.another")).clicked() {
                let p = self.provider;
                self.start_over();
                self.provider = p;
            }
            if ui.button(t("payroll.done.open_feed")).clicked() {
                self.link_open("banking.feed");
            }
            if ui.button(t("payroll.done.open_history")).clicked() {
                self.show_history();
            }
        });
    }

    fn link_open(&self, id: &'static str) {
        self.link_ref()
            .push(CompanyEvent::OpenWindow(CommandId::new(id)));
    }

    fn history_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        heading(ui, tokens, "payroll.history.heading");
        if ui
            .checkbox(&mut self.unposted_only, t("payroll.history.unposted_only"))
            .changed()
        {
            self.load_history();
        }
        if let Some(n) = self.reversed {
            note(
                ui,
                tokens,
                &t_count(
                    "payroll.history.reversed",
                    usize::try_from(n).unwrap_or(usize::MAX),
                ),
            );
        }
        let Some(h) = self.history.clone() else {
            return;
        };
        if h.sessions.is_empty() {
            note(ui, tokens, &t("payroll.history.empty"));
            if ui.button(t("payroll.history.import")).clicked() {
                self.view = View::Import;
            }
            return;
        }
        egui::Grid::new("payroll.history")
            .num_columns(6)
            .striped(true)
            .show(ui, |ui| {
                for k in [
                    "payroll.history.provider",
                    "payroll.history.period",
                    "payroll.history.check_date",
                    "payroll.history.status",
                    "payroll.history.entries",
                ] {
                    ui.strong(t(k));
                }
                ui.label("");
                ui.end_row();
                for s in &h.sessions {
                    let provider = s
                        .provider
                        .as_deref()
                        .and_then(Provider::from_engine_key)
                        .unwrap_or(Provider::GenericCsv);
                    ui.label(t(&format!("payroll.provider.{}", provider.key())));
                    ui.label(match (s.period_start, s.period_end) {
                        (Some(a), Some(b)) => format!("{} – {}", show_date(a), show_date(b)),
                        _ => String::new(),
                    });
                    ui.label(s.check_date.map(show_date).unwrap_or_default());
                    let status = match s.status {
                        SessionStatus::Posted => "payroll.status.posted_short",
                        other => match other.as_wire() {
                            "uploaded" => "payroll.status.uploaded",
                            "mapped" => "payroll.status.mapped",
                            "validated" => "payroll.status.validated",
                            "failed" => "payroll.status.failed",
                            _ => "payroll.status.cancelled",
                        },
                    };
                    // QA-12: a posted import says when (the local date).
                    let posted_on = (s.status == SessionStatus::Posted)
                        .then(|| s.posted_at.as_deref().and_then(posted_day))
                        .flatten();
                    // QA-15: a draft whose post's answer was lost may be
                    // in the books: it says so and offers Check status.
                    let unconfirmed = s.status.is_unposted_draft() && self.is_unsettled(s.id);
                    ui.label(match posted_on {
                        Some(d) => t_with("payroll.status.posted_on", &[("date", &show_date(d))]),
                        None if unconfirmed => t("payroll.status.unconfirmed"),
                        None => t(status),
                    });
                    ui.label(s.entries.len().to_string());
                    ui.horizontal(|ui| {
                        if unconfirmed && ui.button(t("payroll.check_status")).clicked() {
                            self.check_draft_status(s.id);
                        }
                        // S2: no new reverse while one is outstanding.
                        if s.status == SessionStatus::Posted
                            && ui
                                .add_enabled(
                                    !self.reverse_outstanding(),
                                    egui::Button::new(t("payroll.history.reverse")),
                                )
                                .clicked()
                        {
                            self.password = None;
                            self.reversing = Some((s.id, t("payroll.history.reason_default")));
                        }
                        // B1: this window's own draft is never discarded
                        // while its post is out or unconfirmed.
                        if s.status.is_unposted_draft()
                            && ui
                                .add_enabled(
                                    self.may_discard_session(s.id),
                                    egui::Button::new(t("payroll.history.discard")),
                                )
                                .clicked()
                        {
                            self.discard(s.id);
                        }
                    });
                    ui.end_row();
                }
            });
        if self.asking_password.is_some() {
            // Where Reverse was clicked (QA-3).
            self.password_ui(ui, tokens);
        } else if let Some((id, mut reason)) = self.reversing.clone() {
            ui.add_space(8.0);
            // S2: the reason is part of the keyed body; it is locked while
            // an answer is outstanding (Try again resends it as sent).
            let open = !self.reverse_outstanding();
            ui.horizontal(|ui| {
                let l = ui.label(t("payroll.history.reason"));
                ui.add_enabled(
                    open,
                    egui::TextEdit::singleline(&mut reason).desired_width(280.0),
                )
                .labelled_by(l.id);
                if ui
                    .add_enabled(
                        open,
                        egui::Button::new(t("payroll.history.reverse_confirm")),
                    )
                    .clicked()
                {
                    self.reverse(id, &reason);
                }
                if ui
                    .add_enabled(open, egui::Button::new(t("payroll.cancel")))
                    .clicked()
                {
                    self.reversing = None;
                    return;
                }
                if let Some(r) = &mut self.reversing {
                    r.1 = reason;
                }
            });
        }
    }

    fn dialogs_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if let Some(overlaps) = self.overlaps.clone() {
            ui.add_space(10.0);
            heading(ui, tokens, "payroll.overlap.heading");
            note(ui, tokens, &t("payroll.overlap.text"));
            for o in &overlaps {
                let period = o.period.map_or_else(
                    || o.check_date.map(show_date).unwrap_or_default(),
                    |(a, b)| format!("{} – {}", show_date(a), show_date(b)),
                );
                ui.label(t_with(
                    "payroll.overlap.item",
                    &[
                        ("period", &period),
                        (
                            "posted",
                            &o.posted_on.as_deref().map(posted_text).unwrap_or_default(),
                        ),
                    ],
                ));
                if !o.matching_dates.is_empty() {
                    let d: Vec<String> = o.matching_dates.iter().map(|d| show_date(*d)).collect();
                    ui.label(t_with("payroll.overlap.dates", &[("dates", &d.join(", "))]));
                }
            }
            ui.horizontal(|ui| {
                if ui.button(t("payroll.overlap.post_anyway")).clicked() {
                    self.post_anyway();
                }
                if ui.button(t("payroll.cancel")).clicked() {
                    self.overlaps = None;
                }
            });
        }
    }

    /// The closing-date password prompt (QA-3, QA-7): drawn beside the
    /// button that asked (Post in the bottom bar, Reverse in History),
    /// focused and scrolled into view when it opens; the closing date in
    /// its text; a wrong password said beside the field; Enter submits.
    fn password_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(lock) = self.asking_password else {
            return;
        };
        ui.add_space(6.0);
        heading(ui, tokens, "payroll.closed.heading");
        let text = match lock {
            Some(d) => t_with("payroll.closed.text", &[("date", &show_date(d))]),
            None => t("payroll.closed.text_undated"),
        };
        note(ui, tokens, &text);
        ui.horizontal_wrapped(|ui| {
            let l = ui.label(t("payroll.closed.password"));
            let r = ui
                .add(
                    egui::TextEdit::singleline(&mut self.password_text)
                        .password(true)
                        .desired_width(200.0),
                )
                .labelled_by(l.id);
            if std::mem::take(&mut self.password_focus) {
                r.request_focus();
                r.scroll_to_me(Some(egui::Align::Center));
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button(t("payroll.closed.continue")).clicked() || enter {
                self.give_password();
            }
            if ui.button(t("payroll.cancel")).clicked() {
                self.asking_password = None;
                self.password_text.clear();
                if matches!(
                    self.trouble,
                    Some(Trouble::PasswordWrong | Trouble::PasswordWait)
                ) {
                    self.trouble = None;
                }
            }
            match self.trouble {
                Some(Trouble::PasswordWrong) => warn(ui, tokens, &t("payroll.password_wrong")),
                Some(Trouble::PasswordWait) => warn(ui, tokens, &t("payroll.password_wait")),
                _ => {}
            }
        });
        ui.add_space(6.0);
    }

    /// The backup before a post (Q-P30h), next to Post: progress with a
    /// way to stop, or why it failed with Try again. There is no way to
    /// post without it.
    fn backup_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.step != Step::Preview {
            return;
        }
        // After a lost answer the earlier post may have landed: never say
        // "nothing was posted" then (Check status says which).
        let unsettled = self.post_unsettled;
        if self.backing_up() {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                crate::screens::company::form::progress_line(
                    ui,
                    tokens,
                    &t("payroll.backup.running"),
                );
                if ui.button(t("payroll.backup.stop")).clicked() {
                    self.stop_backup();
                }
            });
            ui.add_space(6.0);
            return;
        }
        if self.backup_stopped {
            ui.add_space(6.0);
            note(
                ui,
                tokens,
                &t(if unsettled {
                    "payroll.backup.stopped_unsettled"
                } else {
                    "payroll.backup.stopped"
                }),
            );
        }
        let Some(e) = &self.backup_failed else {
            return;
        };
        let no_passphrase = matches!(e, cl_company::CompanyError::NoPassphrase);
        ui.add_space(6.0);
        warn(
            ui,
            tokens,
            &t(if unsettled {
                "payroll.backup.failed_unsettled"
            } else {
                "payroll.backup.failed"
            }),
        );
        crate::screens::company::form::inline_error(ui, tokens, e);
        ui.horizontal(|ui| {
            if ui.button(t("payroll.try_again")).clicked() {
                self.backup_try_again();
            }
            if no_passphrase && ui.button(t("payroll.backup.open_backup")).clicked() {
                self.link_open("file.backup");
            }
        });
        ui.add_space(6.0);
    }

    fn buttons_ui(&mut self, ui: &mut Ui) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if self.view == View::History || self.step == Step::Done {
                if ui.button(t("payroll.close")).clicked() {
                    self.link_ref()
                        .push(CompanyEvent::CloseWindow(CommandId::new(super::KIND)));
                }
                return;
            }
            let primary = if self.step == Step::Preview {
                let n = self.preview.as_ref().map_or(0, |p| p.entries.len());
                t_count("payroll.post", n)
            } else {
                t("payroll.next")
            };
            let enabled =
                self.can_next() && self.overlaps.is_none() && self.asking_password.is_none();
            let r = ui
                .add_enabled_ui(enabled, |ui| crate::widgets::primary_button(ui, &primary))
                .inner
                .on_hover_text(shortcut_hint("form.save_new"));
            if r.clicked() {
                self.next();
            }
            // B1: nothing discards or leaves the import while a post or
            // reverse is out, or a post's answer was lost (QA-15: Check
            // status settles it first).
            let settled = !self.in_flight() && !self.post_unsettled;
            if ui
                .add_enabled(settled, egui::Button::new(t("payroll.cancel")))
                .clicked()
            {
                self.cancel();
            }
            // Cancel stops a backup before posting; Back waits for it.
            if self.step != Step::Choose
                && ui
                    .add_enabled(
                        settled && !self.backing_up(),
                        egui::Button::new(t("payroll.back")),
                    )
                    .on_hover_text(shortcut_hint("form.step_back"))
                    .clicked()
            {
                self.back();
            }
            // S4: back to Match columns from a later step, even when the
            // saved layout was applied without it.
            if self.provider.mode() == FileMode::EmployeeLevel
                && matches!(self.step, Step::Accounts | Step::Check | Step::Preview)
                && ui
                    .add_enabled(
                        self.can_match_again(),
                        egui::Button::new(t("payroll.map.match_again")),
                    )
                    .clicked()
            {
                self.match_again();
            }
        });
    }
}

/// `key` with `{n}`, or its `.one` form for exactly one.
pub(crate) fn t_count(key: &str, n: usize) -> String {
    let one = format!("{key}.one");
    if n == 1 && crate::i18n::has(&one) {
        t(&one)
    } else {
        t_with(key, &[("n", &n.to_string())])
    }
}

/// The engine's non-blocking check messages grouped by kind, in the order
/// first seen: `(code, rows)` (0 rows: a note about the whole file).
pub(crate) fn note_groups(messages: &[cl_api::payroll::ValidationMessage]) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for m in messages.iter().filter(|m| !m.is_error) {
        let i = if let Some(i) = out.iter().position(|(c, _)| *c == m.code) {
            i
        } else {
            out.push((m.code.clone(), 0));
            out.len() - 1
        };
        if m.row_number.is_some() {
            out[i].1 += 1;
        }
    }
    out
}

/// A note's reason in plain words (never the engine's message).
fn note_reason(code: &str) -> String {
    let k = format!("payroll.note.{code}");
    if crate::i18n::has(&k) {
        t(&k)
    } else {
        t("payroll.note.other")
    }
}

/// An amount in a preview column, right-aligned so the figures line up
/// (QA-11); the column is at least as wide as a large amount.
fn amount_cell(ui: &mut Ui, text: RichText) {
    ui.allocate_ui_with_layout(
        egui::vec2(110.0, ui.spacing().interact_size.y),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.set_min_width(110.0);
            ui.label(text);
        },
    );
}

/// The height of a column list: every column (plus "none") when it fits in
/// most of the window, else the most that do (the list scrolls).
fn column_list_height(ui: &Ui, columns: usize) -> f32 {
    list_height(ui, columns.saturating_add(1))
}

/// A drop-down list tall enough for `items` rows, capped to the window.
#[allow(clippy::float_arithmetic)] // pixels for egui, never money (M10)
fn list_height(ui: &Ui, items: usize) -> f32 {
    let row = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
    let rows = u16::try_from(items).unwrap_or(u16::MAX);
    let all = f32::from(rows) * row + ui.spacing().menu_margin.sum().y;
    let room = ui.ctx().content_rect().height() * 0.45;
    all.min(room).max(ui.spacing().combo_height)
}

/// The day of the engine's `postedAt` (an ISO timestamp, U-244) on this
/// computer's clock, as the Audit Trail shows it (QA-4: the UTC day was
/// shown); a bare date reads as itself.
pub(crate) fn posted_day(s: &str) -> Option<cl_core::CalendarDate> {
    if let Ok(ts) = cl_core::Timestamp::parse_wire(s) {
        let d = ts.as_utc().with_timezone(&chrono::Local).date_naive();
        return cl_core::CalendarDate::from_naive(d).ok();
    }
    cl_core::CalendarDate::parse_wire(s.get(..10)?).ok()
}

/// An engine time as the owner's local date (MM/DD/YYYY); text that isn't
/// a time is shown as it came.
fn posted_text(s: &str) -> String {
    posted_day(s).map_or_else(|| s.to_string(), show_date)
}

/// Rows shown by number in the undated-rows message before "and N more".
const UNDATED_ROWS_SHOWN: usize = 10;

/// 422 `PAYROLL_DATE_INVALID` in plain words: the file's row numbers only
/// (never cell text).
pub(crate) fn undated_rows_text(rows: &[u64]) -> String {
    if rows.is_empty() {
        return t("payroll.refused.dates");
    }
    let shown = rows
        .iter()
        .take(UNDATED_ROWS_SHOWN)
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let list = match rows.len().checked_sub(UNDATED_ROWS_SHOWN) {
        Some(more) if more > 0 => t_with(
            "payroll.refused.dates_more",
            &[("rows", &shown), ("n", &more.to_string())],
        ),
        _ => shown,
    };
    t_with("payroll.refused.dates_rows", &[("rows", &list)])
}
