//! The application shell: title bar, menu bar, left rail, tab strip, window
//! body, status bar, command palette and shortcut overlay (DESIGN.md "Window
//! anatomy"; the chrome follows `docs/design/mockups/Home.html`).
//!
//! [`Shell`] owns all UI state. Menus, the rail, the palette and the keyboard
//! all do the same thing: queue a command id. [`Shell::dispatch`] is the only
//! place a command takes effect, so every entry point behaves alike.

pub mod menu;
pub mod palette;
pub mod rail;
pub mod shortcuts;
pub mod status_bar;
pub mod tabs;
pub mod title_bar;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cl_core::CalendarDate;
use cl_model::{COMMANDS, Command, CommandId, Keymap, Scope, commands};
use eframe::egui::{self, Color32, Context, Frame, Margin, ViewportBuilder, ViewportCommand};

use crate::about::AboutDialog;
use crate::errors::{self, ErrorDialog};
use crate::i18n::t;
use crate::input;
use crate::prefs::{self, UiPrefs, UiStore};
use crate::theme::{self, Tokens};
use crate::windows::{
    CloseOutcome, Placeholder, PromptAnswer, QuitProgress, Screen, SessionLayout, WindowId, Windows,
};

/// Below this logical width the menu bar collapses to one button and the rail
/// becomes a toggled panel, so the shell works at the minimum window size and
/// at 200% zoom.
pub const NARROW_WIDTH: f32 = 760.0;

/// The smallest window the app opens (points before zoom). At 200% zoom this
/// is 480 x 320 egui points, which the narrow layout is built for (Q-P06e).
pub const MIN_WINDOW: [f32; 2] = [960.0, 640.0];

/// The company shown in the title bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanyBanner {
    /// Company name.
    pub name: String,
    /// "Books closed through" date, when a closing date is set.
    pub closed_through: Option<CalendarDate>,
    /// Accent stripe colour for this company (Q-184), as RGB.
    pub accent: [u8; 3],
}

/// Counts shown next to rail entries. `None` hides the count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RailCounts {
    /// Clients and jobs.
    pub clients: Option<u32>,
    /// Vendors.
    pub vendors: Option<u32>,
    /// Bank transactions waiting for review (the orange badge).
    pub bank_feed: Option<u32>,
}

/// The text parts of the status bar that come from outside the shell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusInfo {
    /// Signed-in user and role, e.g. `Kurt - Admin`; `None` before sign-in.
    pub user: Option<String>,
    /// Mode text, e.g. `Single-user, this computer`.
    pub mode: Option<String>,
    /// AI line, e.g. `AI: off`; `None` hides it.
    pub ai: Option<String>,
}

/// What the status bar says about the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostView {
    /// No supervisor is attached (for example the demo).
    Unavailable,
    /// First start in progress.
    Starting,
    /// Running.
    Running,
    /// A program is being restarted.
    Restarting,
    /// Gave up; the string is a plain-language reason.
    Failed(String),
    /// Shut down.
    Stopped,
}

/// Something that can say how the engine is doing (the supervisor, or a stub
/// in tests). Must be cheap: it is called every frame.
pub trait HostProbe: Send + Sync {
    /// The current view.
    fn view(&self) -> HostView;
}

/// Construction options.
#[derive(Default)]
pub struct ShellOptions {
    /// Where preferences and tab layouts are kept; `None` keeps nothing.
    pub store: Option<UiStore>,
    /// Key for the saved tab layout (a company id; `default` before P08).
    pub company_key: String,
    /// Host log attached to "Copy details".
    pub log_path: Option<PathBuf>,
    /// Engine status source.
    pub host: Option<Arc<dyn HostProbe>>,
    /// Fill the shell with the synthetic Ozark Ridge sample from the mockups.
    pub demo: bool,
    /// Start with this theme instead of the remembered one (screenshots).
    pub theme: Option<theme::ThemeChoice>,
    /// Start at this zoom instead of the remembered one (screenshots).
    pub zoom_percent: Option<u16>,
}

struct Toast {
    text: String,
    until: f64,
}

/// All shell state.
pub struct Shell {
    /// Remembered theme and zoom.
    pub prefs: UiPrefs,
    store: Option<UiStore>,
    company_key: String,
    log_path: Option<PathBuf>,
    keymap: Keymap,
    /// The open windows.
    pub windows: Windows,
    /// Title bar content; `None` shows "No company open".
    pub company: Option<CompanyBanner>,
    /// Rail counts.
    pub counts: RailCounts,
    /// Status bar text.
    pub status: StatusInfo,
    host: Option<Arc<dyn HostProbe>>,
    /// Command palette state.
    pub palette: palette::State,
    shortcuts_open: bool,
    about: AboutDialog,
    error: Option<ErrorDialog>,
    toast: Option<Toast>,
    pending: Vec<CommandId>,
    rail_open_when_narrow: bool,
    today: CalendarDate,
    fixed_today: bool,
    allow_close: bool,
    last_saved_layout: Option<SessionLayout>,
    applied_zoom: Option<u16>,
    applied_theme: Option<theme::ThemeChoice>,
}

impl Shell {
    /// Build the shell. Fonts and styles are installed by [`Shell::install`].
    ///
    /// # Panics
    /// Only if the built-in command table has a shortcut conflict, which a
    /// unit test rules out.
    #[must_use]
    pub fn new(options: ShellOptions) -> Shell {
        let mut prefs = options
            .store
            .as_ref()
            .map(UiStore::load_prefs)
            .unwrap_or_default();
        if let Some(theme) = options.theme {
            prefs.theme = theme;
        }
        if let Some(zoom) = options.zoom_percent {
            prefs.zoom_percent = prefs::nearest_step(zoom);
        }
        // The table is checked for conflicts by a unit test, so this cannot
        // fail in a shipped build.
        let keymap = Keymap::new(COMMANDS).expect("the command table has no shortcut conflicts");
        let mut shell = Shell {
            prefs,
            store: options.store,
            company_key: if options.company_key.is_empty() {
                "default".into()
            } else {
                options.company_key
            },
            log_path: options.log_path,
            keymap,
            windows: Windows::new(),
            company: None,
            counts: RailCounts::default(),
            status: StatusInfo::default(),
            host: options.host,
            palette: palette::State::default(),
            shortcuts_open: false,
            about: AboutDialog::default(),
            error: None,
            toast: None,
            pending: Vec::new(),
            rail_open_when_narrow: false,
            today: today(),
            fixed_today: false,
            allow_close: false,
            last_saved_layout: None,
            applied_zoom: None,
            applied_theme: None,
        };
        if options.demo {
            shell.load_demo();
        } else {
            shell.restore_layout_or_home();
        }
        shell
    }

    /// Install fonts and both styles. Call once with the egui context.
    pub fn install(ctx: &Context) {
        crate::fonts::install(ctx);
        theme::apply(ctx, theme::ThemeChoice::Auto);
    }

    /// The mockup's synthetic company (Ozark Ridge Landscaping LLC) with its
    /// open windows, for screenshots and `--demo`. All data is synthetic.
    fn load_demo(&mut self) {
        self.company = Some(CompanyBanner {
            name: "Ozark Ridge Landscaping LLC".into(),
            closed_through: CalendarDate::from_ymd(2026, 9, 30).ok(),
            accent: [0x23, 0x6B, 0xB5],
        });
        if let Ok(d) = CalendarDate::from_ymd(2026, 11, 4) {
            self.today = d;
            self.fixed_today = true;
        }
        self.counts = RailCounts {
            clients: Some(64),
            vendors: Some(38),
            bank_feed: Some(37),
        };
        self.status = StatusInfo {
            user: Some("Kurt \u{b7} Admin".into()),
            mode: Some("Single-user \u{b7} this computer".into()),
            ai: Some("AI: Ollama, on this computer".into()),
        };
        let mk = |id: &str| commands::find(id).map_or_else(|| CommandId::new("nav.home"), |c| c.id);
        self.windows.open_new(
            mk("nav.home"),
            Box::new(Placeholder::for_command(mk("nav.home"))),
        );
        self.windows.open_new(
            mk("vendors.write_check"),
            Box::new(
                Placeholder::for_command(mk("vendors.write_check"))
                    .with_title("Write Check \u{b7} #4418")
                    .with_dirty(true),
            ),
        );
        self.windows.open_new(
            mk("banking.register"),
            Box::new(
                Placeholder::for_command(mk("banking.register"))
                    .with_title("Register \u{b7} Operating"),
            ),
        );
        self.windows.open_new(
            mk("banking.feed"),
            Box::new(Placeholder::for_command(mk("banking.feed"))),
        );
        if let Some(first) = self.windows.all().first().map(|w| w.id) {
            self.windows.activate(first);
        }
    }

    fn restore_layout_or_home(&mut self) {
        let layout = self
            .store
            .as_ref()
            .and_then(|s| s.load_layout(&self.company_key));
        let restored = layout.is_some_and(|l| {
            self.windows.restore(&l, &|k| {
                Box::new(Placeholder::for_command(k)) as Box<dyn Screen>
            });
            !self.windows.is_empty()
        });
        if !restored {
            self.dispatch_now(CommandId::new("nav.home"));
        }
        self.last_saved_layout = Some(self.windows.snapshot());
    }

    /// Switch to another company's remembered layout (P08 calls this).
    pub fn set_company(&mut self, key: &str, banner: Option<CompanyBanner>) {
        self.save_layout();
        self.company_key = key.to_string();
        self.company = banner;
        let layout = self.store.as_ref().and_then(|s| s.load_layout(key));
        let restored = layout.is_some_and(|l| {
            self.windows.restore(&l, &|k| {
                Box::new(Placeholder::for_command(k)) as Box<dyn Screen>
            })
        });
        if !restored && self.windows.is_empty() {
            self.dispatch_now(CommandId::new("nav.home"));
        }
    }

    /// Persist the tab layout when it changed.
    pub fn save_layout(&mut self) {
        let layout = self.windows.snapshot();
        if self.last_saved_layout.as_ref() == Some(&layout) {
            return;
        }
        if let Some(store) = &self.store
            && let Err(e) = store.save_layout(&self.company_key, &layout)
        {
            tracing::warn!(error = %e, "could not save the window layout");
        }
        self.last_saved_layout = Some(layout);
    }

    /// Today's date for the title bar and date rules.
    #[must_use]
    pub fn today(&self) -> CalendarDate {
        self.today
    }

    /// Show an error dialog for an owner-facing error.
    pub fn show_error(&mut self, error: &dyn cl_core::OwnerFacing) {
        self.error = Some(ErrorDialog::new(
            error,
            errors::fresh_id(),
            self.log_path.as_deref(),
        ));
    }

    /// The error dialog on screen, if any (tests).
    #[must_use]
    pub fn error_dialog(&self) -> Option<&ErrorDialog> {
        self.error.as_ref()
    }

    /// Queue a command to run at the end of this frame.
    pub fn queue(&mut self, id: CommandId) {
        self.pending.push(id);
    }

    /// Window id of the tab in front.
    #[must_use]
    pub fn active_window(&self) -> Option<WindowId> {
        self.windows.active_id()
    }

    /// The keymap scope that applies to the next key press.
    #[must_use]
    pub fn focus_scope(&self) -> Scope {
        self.windows
            .active()
            .map_or(Scope::Global, |w| w.screen.scope())
    }

    /// The keymap.
    #[must_use]
    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    /// True while the shortcut overlay is open.
    #[must_use]
    pub fn shortcuts_open(&self) -> bool {
        self.shortcuts_open
    }

    /// True while About or the privacy notice is open.
    #[must_use]
    pub fn about_open(&self) -> bool {
        self.about.is_open()
    }

    fn modal_open(&self) -> bool {
        self.palette.open
            || self.shortcuts_open
            || self.about.is_open()
            || self.error.is_some()
            || self.windows.prompt().is_some()
    }

    fn toast(&mut self, ctx: &Context, key: &str) {
        let now = ctx.input(|i| i.time);
        self.toast = Some(Toast {
            text: t(key),
            until: now + 4.0,
        });
        ctx.request_repaint_after(Duration::from_secs(4));
    }

    /// Run a command now (outside a frame; tests and start-up).
    pub fn dispatch_now(&mut self, id: CommandId) {
        self.run(None, id);
    }

    fn run(&mut self, ctx: Option<&Context>, id: CommandId) {
        let Some(cmd) = commands::find(id.as_str()) else {
            return;
        };
        if cmd.opens {
            let kind = cmd.id;
            self.windows
                .open_or_focus(kind, || Box::new(Placeholder::for_command(kind)));
            return;
        }
        match id.as_str() {
            "window.next" => self.windows.next(),
            "window.prev" => self.windows.prev(),
            "window.pop_out" => {
                if let Some(w) = self.windows.active_id() {
                    self.windows.pop_out(w);
                }
            }
            "window.zoom_in" => self.prefs.zoom_percent = prefs::zoom_in(self.prefs.zoom_percent),
            "window.zoom_out" => {
                self.prefs.zoom_percent = prefs::zoom_out(self.prefs.zoom_percent);
            }
            "window.zoom_reset" => self.prefs.zoom_percent = prefs::ZOOM_DEFAULT,
            "window.theme" => self.prefs.theme = self.prefs.theme.next(),
            "file.close_window" => {
                if let Some(w) = self.windows.active_id() {
                    self.windows.request_close(w);
                }
            }
            "file.quit" => self.request_quit(ctx),
            "help.palette" => self.palette.toggle(),
            "help.shortcuts" => self.shortcuts_open = !self.shortcuts_open,
            "help.about" => self.about.open_about(),
            "help.privacy" => self.about.open_privacy(),
            "help.current" => {
                if let Some(ctx) = ctx {
                    self.toast(ctx, "toast.help_later");
                }
            }
            _ => self.forward_to_screen(ctx, cmd),
        }
    }

    /// Form and window commands go to the screen in front; commands nothing
    /// handles say so instead of silently doing nothing.
    fn forward_to_screen(&mut self, ctx: Option<&Context>, cmd: &Command) {
        let handled = self
            .windows
            .active_mut()
            .is_some_and(|w| w.screen.command(cmd.id));
        let quiet = cmd.id.as_str().starts_with("form.") || cmd.id.as_str().starts_with("edit.");
        if !handled
            && !quiet
            && let Some(ctx) = ctx
        {
            self.toast(ctx, "toast.not_here");
        }
    }

    fn request_quit(&mut self, ctx: Option<&Context>) {
        if self.windows.begin_quit() == QuitProgress::Ready {
            self.finish_quit(ctx);
        }
    }

    fn finish_quit(&mut self, ctx: Option<&Context>) {
        self.save_layout();
        self.allow_close = true;
        if let Some(ctx) = ctx {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    /// True once a quit was approved (tests).
    #[must_use]
    pub fn quit_approved(&self) -> bool {
        self.allow_close
    }

    /// Bring the window to the front (a second launch asked for focus).
    pub fn focus_app(ctx: &Context) {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn handle_keys(&mut self, ctx: &Context) {
        let pressed = ctx.input(input::pressed_shortcuts);
        if pressed.is_empty() {
            return;
        }
        let modal = self.modal_open();
        let text_focus = ctx.egui_wants_keyboard_input();
        let scope = self.focus_scope();
        let mut handled: Vec<cl_model::Shortcut> = Vec::new();
        for s in pressed {
            let Some(id) = self.keymap.resolve(scope, s) else {
                continue;
            };
            if modal && !matches!(id.as_str(), "help.palette" | "help.shortcuts") {
                continue;
            }
            if text_focus && input::reserved_by_text_fields(s) {
                continue;
            }
            handled.push(s);
            self.run(Some(ctx), id);
        }
        if !handled.is_empty() {
            // The key is ours: keep text fields and focus traversal from also
            // reacting to it.
            ctx.input_mut(|i| {
                i.events.retain(|e| match e {
                    egui::Event::Key { key, modifiers, .. } => match input::key_from_egui(*key) {
                        Some(k) => !handled.contains(&cl_model::Shortcut::new(
                            input::mods_from_egui(*modifiers, k),
                            k,
                        )),
                        None => true,
                    },
                    _ => true,
                });
            });
        }
    }

    fn begin_frame(&mut self, ctx: &Context) {
        if self.applied_theme != Some(self.prefs.theme) {
            theme::apply(ctx, self.prefs.theme);
            self.applied_theme = Some(self.prefs.theme);
        }
        if self.applied_zoom != Some(self.prefs.zoom_percent) {
            ctx.set_zoom_factor(self.prefs.zoom_factor());
            self.applied_zoom = Some(self.prefs.zoom_percent);
            if let Some(store) = &self.store
                && let Err(e) = store.save_prefs(&self.prefs)
            {
                tracing::warn!(error = %e, "could not save preferences");
            }
        }
        if !self.fixed_today {
            self.today = today();
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.request_quit(Some(ctx));
        }
        self.handle_keys(ctx);
        if let Some(toast) = &self.toast
            && ctx.input(|i| i.time) > toast.until
        {
            self.toast = None;
        }
    }

    /// The text of the toast on screen, if any.
    #[must_use]
    pub fn toast_text(&self) -> Option<&str> {
        self.toast.as_ref().map(|t| t.text.as_str())
    }

    /// Draw one frame of the whole shell into the root `ui`.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // Persist prefs changed by commands (theme).
        let before_theme = self.applied_theme;
        self.begin_frame(&ctx);
        if before_theme.is_some()
            && before_theme != self.applied_theme
            && let Some(store) = &self.store
        {
            let _ = store.save_prefs(&self.prefs);
        }
        let narrow = ui.available_width() < NARROW_WIDTH;
        let tokens = Tokens::of(&ctx);

        title_bar::show(ui, self, narrow, &tokens);
        menu::show(ui, self, narrow, &tokens);
        status_bar::show(ui, self, narrow, &tokens);
        if !narrow || self.rail_open_when_narrow {
            rail::show(ui, self, &tokens);
        }
        tabs::show(ui, self, narrow, &tokens);
        self.body(ui, &tokens);
        self.show_popped_out(&ctx, &tokens);
        self.show_overlays(&ctx);

        for id in std::mem::take(&mut self.pending) {
            self.run(Some(&ctx), id);
        }
        self.save_layout();
        if self.toast.is_some() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    /// Show or hide the rail when the window is narrow.
    pub fn toggle_rail(&mut self) {
        self.rail_open_when_narrow = !self.rail_open_when_narrow;
    }

    /// True when the rail is open in narrow mode.
    #[must_use]
    pub fn rail_open_when_narrow(&self) -> bool {
        self.rail_open_when_narrow
    }

    /// The engine status for the status bar.
    #[must_use]
    pub fn host_view(&self) -> HostView {
        self.host
            .as_ref()
            .map_or(HostView::Unavailable, |h| h.view())
    }

    fn body(&mut self, ui: &mut egui::Ui, tokens: &Tokens) {
        let frame = Frame::new()
            .fill(tokens.surface_window)
            .inner_margin(Margin::symmetric(16, 12));
        let active = self.windows.active_id();
        let mut dock: Option<WindowId> = None;
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            let Some(id) = active else {
                ui.add_space(12.0);
                ui.label(t("shell.empty"));
                return;
            };
            let Some(w) = self.windows.get_mut(id) else {
                return;
            };
            if w.popped_out {
                ui.add_space(12.0);
                ui.label(t("shell.popped_out"));
                if ui.button(t("shell.bring_back")).clicked() {
                    dock = Some(id);
                }
            } else {
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| w.screen.ui(ui));
            }
        });
        if let Some(id) = dock {
            self.windows.dock(id);
        }
    }

    /// Windows moved out of the tab strip get their own operating-system
    /// window (egui viewport). Closing that window docks it back (Q-57).
    fn show_popped_out(&mut self, ctx: &Context, tokens: &Tokens) {
        let mut dock: Vec<WindowId> = Vec::new();
        for w in self.windows.all_mut().iter_mut().filter(|w| w.popped_out) {
            let title = format!("{} - ClientLedger", w.title());
            let id = egui::ViewportId::from_hash_of(("cl-window", w.id.raw()));
            let builder = ViewportBuilder::default()
                .with_title(title)
                .with_inner_size([900.0, 640.0])
                .with_min_inner_size(MIN_WINDOW);
            let fill = tokens.surface_window;
            let wid = w.id;
            let screen = &mut w.screen;
            ctx.show_viewport_immediate(id, builder, |ui, _class| {
                egui::CentralPanel::default()
                    .frame(Frame::new().fill(fill).inner_margin(Margin::same(16)))
                    .show(ui, |ui| screen.ui(ui));
                if ui.ctx().input(|i| i.viewport().close_requested()) {
                    dock.push(wid);
                }
            });
        }
        for id in dock {
            self.windows.dock(id);
        }
    }

    fn show_overlays(&mut self, ctx: &Context) {
        if let Some(id) = palette::show(ctx, &mut self.palette, &self.keymap) {
            self.pending.push(id);
        }
        if self.shortcuts_open && shortcuts::show(ctx, self.focus_scope(), &self.keymap) {
            self.shortcuts_open = false;
        }
        self.about.show(ctx);
        if let Some(dialog) = &mut self.error
            && errors::show(ctx, dialog) == errors::DialogAction::Close
        {
            self.error = None;
        }
        self.show_close_prompt(ctx);
    }

    fn show_close_prompt(&mut self, ctx: &Context) {
        let Some(prompt) = self.windows.prompt() else {
            return;
        };
        let title = self
            .windows
            .get(prompt.window)
            .map(crate::windows::Window::title)
            .unwrap_or_default();
        let mut answer: Option<PromptAnswer> = None;
        egui::Window::new(t("window.prompt.title"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(crate::windows::prompt_message(&title));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if crate::widgets::primary_button(ui, &t("window.prompt.save")).clicked() {
                        answer = Some(PromptAnswer::Save);
                    }
                    if ui.button(t("window.prompt.discard")).clicked() {
                        answer = Some(PromptAnswer::Discard);
                    }
                    if ui.button(t("window.prompt.keep")).clicked() {
                        answer = Some(PromptAnswer::Keep);
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = Some(PromptAnswer::Keep);
        }
        if let Some(a) = answer
            && self.windows.answer_prompt(a) == Some(QuitProgress::Ready)
        {
            self.finish_quit(Some(ctx));
        }
    }

    /// Close a window from the UI (tab close button).
    pub fn close_window(&mut self, id: WindowId) -> CloseOutcome {
        self.windows.request_close(id)
    }

    /// Accent colour of the company stripe.
    #[must_use]
    pub fn accent(&self, tokens: &Tokens) -> Color32 {
        self.company.as_ref().map_or(tokens.accent, |c| {
            Color32::from_rgb(c.accent[0], c.accent[1], c.accent[2])
        })
    }
}

/// Today's local date.
#[must_use]
pub fn today() -> CalendarDate {
    use chrono::Datelike;
    let now = chrono::Local::now().date_naive();
    CalendarDate::from_ymd(now.year(), now.month(), now.day())
        .unwrap_or_else(|_| CalendarDate::min())
}

/// `MM/DD/YYYY`.
#[must_use]
pub fn us_date(d: CalendarDate) -> String {
    format!("{:02}/{:02}/{}", d.month(), d.day(), d.year())
}

/// `Wed 11/04/2026`.
#[must_use]
pub fn weekday_date(d: CalendarDate) -> String {
    format!("{} {}", d.weekday(), us_date(d))
}

/// Hint chip text: `Ctrl+Tab` shown as `Ctrl Tab`, as in the mockup.
#[must_use]
pub fn chip_text(s: cl_model::Shortcut) -> String {
    input::shortcut_text(s).replace('+', " ")
}

#[cfg(test)]
mod tests;
