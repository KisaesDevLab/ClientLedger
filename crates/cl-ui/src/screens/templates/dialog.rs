//! The Template dialog (Save as Template and Edit): Name, Group (pick or
//! new), what happens on the dates, How often + Every N, Next date, Ends
//! (never / on / after N times), with the five-date preview. The gate is
//! `cl_model::templates::TemplateDialog::check`; the preview is display
//! only (the engine's next date after the save is what the list shows).

use cl_api::{Frequency, ScheduleEnd, ScheduleMode, Template, TemplateGroup, TemplateSchedule};
use cl_core::CalendarDate;
use cl_core::ids::TemplateGroupId;
use cl_model::templates::{
    self as model, CheckedDialog, DialogProblem, Ends, GroupChoice, Mode, TemplateDialog,
};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::i18n::{t, t_with};
use crate::screens::company::form::DateInput;
use crate::theme::Tokens;

/// What the owner did in the dialog this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DialogClick {
    /// Nothing yet.
    None,
    /// Save with these values (checked).
    Save(CheckedDialog<TemplateGroupId>),
    /// Cancel (or Esc).
    Cancel,
}

/// How the schedule ends, as the radio shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EndKind {
    Never,
    On,
    After,
}

/// The dialog's state.
#[derive(Clone, Debug)]
pub(crate) struct DialogUi {
    /// The window title key.
    pub title: &'static str,
    /// The values.
    pub form: TemplateDialog<TemplateGroupId>,
    /// The groups to pick from.
    pub groups: Vec<TemplateGroup>,
    interval_text: String,
    next: DateInput,
    end_date: DateInput,
    count_text: String,
    end_kind: EndKind,
    new_group: bool,
    new_group_text: String,
    /// The problems of the last Save.
    pub problems: Vec<DialogProblem>,
    today: CalendarDate,
    /// Focus the name once.
    focus_name: bool,
    /// The save is on its way (buttons off).
    pub busy: bool,
}

/// The model's frequency for the engine's.
#[must_use]
pub(crate) fn model_frequency(f: Frequency) -> model::Frequency {
    match f {
        Frequency::Daily => model::Frequency::Daily,
        Frequency::Weekly => model::Frequency::Weekly,
        Frequency::Biweekly => model::Frequency::Biweekly,
        Frequency::Semimonthly => model::Frequency::Semimonthly,
        Frequency::Monthly => model::Frequency::Monthly,
        Frequency::Quarterly => model::Frequency::Quarterly,
        Frequency::Annually => model::Frequency::Annually,
    }
}

/// The engine's frequency for the model's.
#[must_use]
pub(crate) fn api_frequency(f: model::Frequency) -> Frequency {
    match f {
        model::Frequency::Daily => Frequency::Daily,
        model::Frequency::Weekly => Frequency::Weekly,
        model::Frequency::Biweekly => Frequency::Biweekly,
        model::Frequency::Semimonthly => Frequency::Semimonthly,
        model::Frequency::Monthly => Frequency::Monthly,
        model::Frequency::Quarterly => Frequency::Quarterly,
        model::Frequency::Annually => Frequency::Annually,
    }
}

/// The engine schedule of a checked dialog (`None`: Don't remind me).
#[must_use]
pub(crate) fn api_schedule(c: &CheckedDialog<TemplateGroupId>) -> Option<TemplateSchedule> {
    let (auto, s) = c.schedule?;
    Some(TemplateSchedule {
        mode: if auto {
            ScheduleMode::Auto
        } else {
            ScheduleMode::Remind
        },
        frequency: api_frequency(s.frequency),
        interval: s.interval,
        next_date: s.next_date,
        end: match s.ends {
            Ends::On(Some(d)) => ScheduleEnd::On(d),
            Ends::After(Some(n)) => ScheduleEnd::After(n),
            _ => ScheduleEnd::Never,
        },
    })
}

/// The dialog's schedule for an engine one.
#[must_use]
pub(crate) fn model_schedule(s: &TemplateSchedule) -> model::Schedule {
    model::Schedule {
        frequency: model_frequency(s.frequency),
        interval: s.interval,
        next_date: s.next_date,
        ends: match s.end {
            ScheduleEnd::Never => Ends::Never,
            ScheduleEnd::On(d) => Ends::On(Some(d)),
            ScheduleEnd::After(n) => Ends::After(Some(n)),
        },
    }
}

/// The dialog's width when the window has room.
const DIALOG_WIDTH: f32 = 440.0;

/// The unit after "Every N" ("month(s)"); none for twice a month, which
/// runs on the 1st and the 15th whatever N is.
fn unit_key(f: model::Frequency) -> Option<&'static str> {
    match f {
        model::Frequency::Daily => Some("templates.dialog.unit.days"),
        model::Frequency::Weekly => Some("templates.dialog.unit.weeks"),
        model::Frequency::Biweekly => Some("templates.dialog.unit.biweeks"),
        model::Frequency::Semimonthly => None,
        model::Frequency::Monthly => Some("templates.dialog.unit.months"),
        model::Frequency::Quarterly => Some("templates.dialog.unit.quarters"),
        model::Frequency::Annually => Some("templates.dialog.unit.years"),
    }
}

/// "Every 2 weeks", "Monthly on the 31st (or last day)".
#[must_use]
pub(crate) fn how_often_text(s: &model::Schedule) -> String {
    let h = model::how_often(s);
    let n = h.n.to_string();
    t_with(
        h.key.as_str(),
        &[("n", n.as_str()), ("day", h.day.as_deref().unwrap_or(""))],
    )
}

impl DialogUi {
    /// Save as Template: Remind me, monthly, from `next`.
    #[must_use]
    pub(crate) fn new(
        name: String,
        next: CalendarDate,
        auto_allowed: bool,
        groups: Vec<TemplateGroup>,
        today: CalendarDate,
    ) -> DialogUi {
        let form = TemplateDialog::new(name, next, auto_allowed);
        DialogUi {
            title: "templates.dialog.title.new",
            form,
            groups,
            interval_text: "1".into(),
            next: DateInput::new(next),
            end_date: DateInput::new(next),
            count_text: String::new(),
            end_kind: EndKind::Never,
            new_group: false,
            new_group_text: String::new(),
            problems: Vec::new(),
            today,
            focus_name: true,
            busy: false,
        }
    }

    /// Edit: the template's name, group and schedule.
    #[must_use]
    pub(crate) fn edit(t: &Template, groups: Vec<TemplateGroup>, today: CalendarDate) -> DialogUi {
        let auto_allowed = t.kind.is_editable() && t.kind != cl_api::TemplateKind::DailySales;
        let next = t.schedule.map_or(today, |s| s.next_date);
        let mut d = DialogUi::new(t.name.clone(), next, auto_allowed, groups, today);
        d.title = "templates.dialog.title.edit";
        d.form.group = t.group.map_or(GroupChoice::None, GroupChoice::Existing);
        match &t.schedule {
            None => d.form.mode = Mode::Never,
            Some(s) => {
                d.form.mode = match s.mode {
                    ScheduleMode::Auto => Mode::Auto,
                    ScheduleMode::Remind => Mode::Remind,
                };
                d.form.frequency = model_frequency(s.frequency);
                d.form.interval = Some(s.interval);
                d.interval_text = s.interval.to_string();
                match s.end {
                    ScheduleEnd::Never => {}
                    ScheduleEnd::On(e) => {
                        d.end_kind = EndKind::On;
                        d.end_date = DateInput::new(e);
                        d.form.ends = Ends::On(Some(e));
                    }
                    ScheduleEnd::After(n) => {
                        d.end_kind = EndKind::After;
                        d.count_text = n.to_string();
                        d.form.ends = Ends::After(Some(n));
                    }
                }
            }
        }
        d
    }

    /// The typed texts into the form values.
    fn sync(&mut self) {
        self.form.interval = self.interval_text.trim().parse().ok();
        self.form.next_date = self.next.value;
        self.form.ends = match self.end_kind {
            EndKind::Never => Ends::Never,
            EndKind::On => Ends::On(self.end_date.value),
            EndKind::After => Ends::After(self.count_text.trim().parse().ok()),
        };
        if self.new_group {
            self.form.group = GroupChoice::New(self.new_group_text.clone());
        }
    }

    /// Checks the values (the dialog's Save).
    pub(crate) fn save(&mut self) -> Option<CheckedDialog<TemplateGroupId>> {
        self.sync();
        match self.form.check() {
            Ok(c) => {
                self.problems.clear();
                Some(c)
            }
            Err(p) => {
                self.problems = p;
                None
            }
        }
    }

    /// Sets a value from the control channel or a test.
    #[cfg(test)]
    pub(crate) fn set(&mut self, field: &str, value: &str) -> bool {
        match field {
            "name" => self.form.name = value.to_string(),
            "interval" => self.interval_text = value.to_string(),
            "next_date" => {
                self.next.text = value.to_string();
                self.next.reparse(self.today);
            }
            "mode" => {
                let Some(m) = Mode::ALL.into_iter().find(|m| m.name() == value) else {
                    return false;
                };
                self.form.mode = m;
            }
            "frequency" => {
                let Some(f) = model::Frequency::ALL
                    .into_iter()
                    .find(|f| f.name() == value)
                else {
                    return false;
                };
                self.form.frequency = f;
            }
            "ends" => {
                self.end_kind = match value {
                    "never" => EndKind::Never,
                    "on" => EndKind::On,
                    "after" => EndKind::After,
                    _ => return false,
                };
            }
            "end_date" => {
                self.end_date.text = value.to_string();
                self.end_date.reparse(self.today);
            }
            "count" => self.count_text = value.to_string(),
            "new_group" => {
                self.new_group = true;
                self.new_group_text = value.to_string();
            }
            _ => return false,
        }
        self.sync();
        true
    }

    /// The control-channel state (no name: owner data).
    pub(crate) fn state_json(&self) -> Value {
        let mut me = self.clone();
        me.sync();
        let preview: Vec<String> = me
            .form
            .schedule()
            .map(|s| model::next_dates(&s, model::PREVIEW_COUNT))
            .unwrap_or_default()
            .into_iter()
            .map(CalendarDate::to_wire)
            .collect();
        json!({
            "title": self.title,
            "mode": self.form.mode.name(),
            "modes": self.form.modes().iter().map(|m| m.name()).collect::<Vec<_>>(),
            "frequency": self.form.frequency.name(),
            "next_date": self.next.value.map(CalendarDate::to_wire),
            "preview": preview,
            "problems": self.problems.iter().map(|p| p.name()).collect::<Vec<_>>(),
            "busy": self.busy,
        })
    }

    /// Draws the dialog window. It fits the window (960 x 640 at 200 %):
    /// the fields scroll and Save / Cancel stay in view.
    pub(crate) fn show(&mut self, ctx: &egui::Context) -> DialogClick {
        let tokens = Tokens::of(ctx);
        let mut click = DialogClick::None;
        let screen = ctx.content_rect();
        let width = (screen.width() - 48.0).clamp(240.0, DIALOG_WIDTH);
        egui::Window::new(t(self.title))
            .id(egui::Id::unique("templates/dialog"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_height(screen.height() - 48.0)
            .show(ctx, |ui| {
                ui.set_width(width);
                click = self.body(ui, &tokens);
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !self.busy {
            return DialogClick::Cancel;
        }
        click
    }

    fn body(&mut self, ui: &mut Ui, tokens: &Tokens) -> DialogClick {
        // Room for the buttons under the scrolling fields.
        let reserve = 44.0 + 2.0 * ui.spacing().item_spacing.y + 8.0;
        let fields_height = (ui.available_height() - reserve).max(80.0);
        egui::ScrollArea::vertical()
            .id_salt("templates/dialog/scroll")
            .max_height(fields_height)
            .auto_shrink([false, true])
            .show(ui, |ui| self.fields(ui, tokens));
        ui.add_space(8.0);
        let mut click = DialogClick::None;
        ui.horizontal(|ui| {
            let save = ui
                .add_enabled_ui(!self.busy, |ui| {
                    crate::widgets::primary_button(ui, &t("templates.dialog.save"))
                })
                .inner;
            if save.clicked()
                && let Some(c) = self.save()
            {
                click = DialogClick::Save(c);
            }
            if ui
                .add_enabled(
                    !self.busy,
                    egui::Button::new(t("templates.dialog.cancel"))
                        .min_size(egui::Vec2::new(0.0, 44.0)),
                )
                .clicked()
            {
                click = DialogClick::Cancel;
            }
        });
        click
    }

    fn fields(&mut self, ui: &mut Ui, tokens: &Tokens) {
        egui::Grid::new("templates/dialog/grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                let l = ui.label(t("templates.dialog.name"));
                let name_width = (ui.available_width() - 8.0).clamp(120.0, 280.0);
                let r = ui
                    .add(egui::TextEdit::singleline(&mut self.form.name).desired_width(name_width))
                    .labelled_by(l.id);
                if std::mem::take(&mut self.focus_name) {
                    r.request_focus();
                }
                ui.end_row();
                self.group_row(ui);
                ui.end_row();
                // Each radio is named by its own words ("Remind me"), not by
                // the row label, so screen readers tell them apart.
                ui.label(t("templates.dialog.mode"));
                ui.vertical(|ui| {
                    for m in self.form.modes() {
                        ui.radio_value(&mut self.form.mode, m, t(m.key().as_str()));
                    }
                });
                ui.end_row();
                if self.form.mode != Mode::Never {
                    self.schedule_rows(ui);
                }
            });
        self.sync();
        if let Some(s) = self.form.schedule() {
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    RichText::new(how_often_text(&s))
                        .size(12.0)
                        .color(tokens.text_secondary),
                )
                .wrap(),
            );
            let dates: Vec<String> = model::next_dates(&s, model::PREVIEW_COUNT)
                .into_iter()
                .map(crate::shell::us_date)
                .collect();
            ui.add(
                egui::Label::new(
                    RichText::new(t_with(
                        "templates.dialog.preview",
                        &[("dates", dates.join(", ").as_str())],
                    ))
                    .size(12.0)
                    .color(tokens.text_secondary),
                )
                .wrap(),
            );
        }
        for p in &self.problems {
            ui.add(
                egui::Label::new(RichText::new(t(p.key().as_str())).color(tokens.warn_text)).wrap(),
            );
        }
    }

    fn group_row(&mut self, ui: &mut Ui) {
        let l = ui.label(t("templates.dialog.group"));
        ui.vertical(|ui| {
            let shown = if self.new_group {
                t("templates.dialog.group.new")
            } else {
                match &self.form.group {
                    GroupChoice::Existing(id) => self
                        .groups
                        .iter()
                        .find(|g| g.id == *id)
                        .map(|g| g.name.clone())
                        .unwrap_or_default(),
                    _ => t("templates.dialog.group.none"),
                }
            };
            let mut picked: Option<Option<TemplateGroupId>> = None;
            let mut new = false;
            egui::ComboBox::from_id_salt("templates/dialog/group")
                .width(280.0)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(false, t("templates.dialog.group.none"))
                        .clicked()
                    {
                        picked = Some(None);
                    }
                    for g in &self.groups {
                        if ui.selectable_label(false, &g.name).clicked() {
                            picked = Some(Some(g.id));
                        }
                    }
                    if ui
                        .selectable_label(false, t("templates.dialog.group.new"))
                        .clicked()
                    {
                        new = true;
                    }
                })
                .response
                .labelled_by(l.id);
            if let Some(p) = picked {
                self.new_group = false;
                self.form.group = p.map_or(GroupChoice::None, GroupChoice::Existing);
            }
            if new {
                self.new_group = true;
            }
            if self.new_group {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.new_group_text)
                        .hint_text(t("templates.dialog.group.new_hint"))
                        .desired_width(280.0),
                );
                r.labelled_by(l.id);
            }
        });
    }

    fn schedule_rows(&mut self, ui: &mut Ui) {
        let l = ui.label(t("templates.dialog.how_often"));
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("templates/dialog/frequency")
                .width(150.0)
                .selected_text(t(self.form.frequency.key().as_str()))
                .show_ui(ui, |ui| {
                    for f in model::Frequency::ALL {
                        ui.selectable_value(&mut self.form.frequency, f, t(f.key().as_str()));
                    }
                })
                .response
                .labelled_by(l.id);
            let e = ui.label(t("templates.dialog.every"));
            ui.add(egui::TextEdit::singleline(&mut self.interval_text).desired_width(48.0))
                .labelled_by(e.id);
            if let Some(unit) = unit_key(self.form.frequency) {
                ui.label(t(unit));
            }
        });
        ui.end_row();
        let l = ui.label(t("templates.dialog.next_date"));
        date_input(ui, &mut self.next, self.today, l.id);
        ui.end_row();
        let l = ui.label(t("templates.dialog.ends"));
        ui.vertical(|ui| {
            ui.radio_value(
                &mut self.end_kind,
                EndKind::Never,
                t("templates.dialog.ends.never"),
            );
            ui.horizontal(|ui| {
                ui.radio_value(
                    &mut self.end_kind,
                    EndKind::On,
                    t("templates.dialog.ends.on"),
                );
                if self.end_kind == EndKind::On {
                    date_input(ui, &mut self.end_date, self.today, l.id);
                }
            });
            ui.horizontal(|ui| {
                ui.radio_value(
                    &mut self.end_kind,
                    EndKind::After,
                    t("templates.dialog.ends.after"),
                );
                if self.end_kind == EndKind::After {
                    ui.add(egui::TextEdit::singleline(&mut self.count_text).desired_width(56.0))
                        .labelled_by(l.id);
                    ui.label(t("templates.dialog.ends.times"));
                }
            });
        });
        ui.end_row();
    }
}

pub(super) fn date_input(ui: &mut Ui, input: &mut DateInput, today: CalendarDate, label: egui::Id) {
    let before = input.text.clone();
    let r = ui
        .add(
            egui::TextEdit::singleline(&mut input.text)
                .desired_width(110.0)
                .font(egui::TextStyle::Monospace),
        )
        .labelled_by(label);
    if r.changed() {
        input.typed(&before, today);
    }
    if r.lost_focus() {
        input.settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, day).expect("date")
    }

    #[test]
    fn typed_values_become_the_engine_schedule() {
        let mut dlg = DialogUi::new(
            "Shop rent".into(),
            d(2026, 12, 1),
            true,
            Vec::new(),
            d(2026, 11, 4),
        );
        assert!(dlg.set("frequency", "weekly"));
        assert!(dlg.set("interval", "2"));
        assert!(dlg.set("ends", "after"));
        assert!(dlg.set("count", "3"));
        assert!(!dlg.set("mode", "sometimes"));
        let c = dlg.save().expect("ok");
        let s = api_schedule(&c).expect("schedule");
        assert_eq!(
            (s.mode, s.frequency, s.interval, s.end),
            (
                ScheduleMode::Remind,
                Frequency::Weekly,
                2,
                ScheduleEnd::After(3)
            )
        );
        assert_eq!(model_schedule(&s).interval, 2);
        let v = dlg.state_json();
        assert_eq!(v["preview"].as_array().map(Vec::len), Some(3));
        assert!(!v.to_string().contains("Shop"));
        assert!(dlg.set("interval", "0"));
        assert!(dlg.save().is_none());
        assert_eq!(dlg.problems, vec![DialogProblem::Interval]);
        assert!(dlg.set("mode", "never"));
        assert!(dlg.save().is_some_and(|c| api_schedule(&c).is_none()));
        for f in Frequency::ALL {
            assert_eq!(api_frequency(model_frequency(f)), f);
        }
    }

    #[test]
    fn plain_how_often_words() {
        let s = model::Schedule {
            frequency: model::Frequency::Monthly,
            interval: 1,
            next_date: d(2026, 1, 31),
            ends: Ends::Never,
        };
        assert_eq!(how_often_text(&s), "Monthly on the 31st (or last day)");
        let s = model::Schedule {
            frequency: model::Frequency::Weekly,
            interval: 2,
            ..s
        };
        assert_eq!(how_often_text(&s), "Every 2 weeks");
    }

    #[test]
    fn the_dialog_draws_and_esc_cancels() {
        let ctx = egui::Context::default();
        let mut dlg = DialogUi::new(
            "Shop rent".into(),
            d(2026, 12, 1),
            false,
            Vec::new(),
            d(2026, 11, 4),
        );
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert_eq!(dlg.show(ui.ctx()), DialogClick::None);
        });
        out.textures_delta.clear();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..egui::RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            assert_eq!(dlg.show(ui.ctx()), DialogClick::Cancel);
        });
        out.textures_delta.clear();
        assert_eq!(dlg.state_json()["modes"], json!(["remind", "never"]));
    }
}
