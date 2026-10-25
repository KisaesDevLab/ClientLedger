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
use cl_model::{COMMANDS, Command, CommandId, EnterMode, Keymap, Scope, commands};
use eframe::egui::{self, Color32, Context, Frame, Margin, ViewportBuilder, ViewportCommand};

use crate::about::AboutDialog;
use crate::company::{CompanyEvent, CompanyLink};
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
    /// The engine link for the company screens (P08). `None` with `demo`
    /// makes a demo link in a temporary folder.
    pub company: Option<Arc<CompanyLink>>,
}

struct Toast {
    /// Its i18n key.
    key: String,
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
    run_only: bool,
    company_link: Option<Arc<CompanyLink>>,
    /// The open company's tenant and name (for Back Up, Delete, layouts).
    open_tenant: Option<(cl_core::ids::TenantId, String)>,
    /// The window to bring back after a close prompt that a company switch
    /// raised (the Open Company window).
    return_after_prompt: Option<WindowId>,
    /// File > Close Company is waiting for a close prompt to be answered.
    close_company_after_prompt: bool,
    /// Remembered grid column widths (`grid-widths.json`), shared with the
    /// grid screens through the egui context.
    widths: crate::grid_env::WidthsHandle,
    /// The close prompt whose Save button already got focus.
    prompt_focused: Option<crate::windows::Prompt>,
    /// Running with `--demo`: developer-only commands and screens exist.
    demo: bool,
    /// The open company's templates catch-up (P15).
    runner: crate::templates_runner::TemplatesRunner,
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
        let keymap = Keymap::with_mode(COMMANDS, EnterMode::from_pref(prefs.enter_moves_fields))
            .expect("the command table has no shortcut conflicts");
        let company_link = options.company.or_else(|| {
            options.demo.then(|| {
                let dir =
                    std::env::temp_dir().join(format!("clientledger-demo-{}", std::process::id()));
                Arc::new(CompanyLink::demo(&dir))
            })
        });
        let widths = crate::grid_env::WidthsHandle::default();
        if let Some(store) = &options.store
            && let Ok(mut w) = widths.lock()
        {
            w.book = store.load_grid_widths();
            w.registers = store.load_register_prefs();
            w.panels = crate::prefs::PanelPrefsBook::load(store.dir());
        }
        let mut shell = Shell {
            widths,
            prompt_focused: None,
            demo: options.demo,
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
            run_only: false,
            company_link,
            open_tenant: None,
            return_after_prompt: None,
            close_company_after_prompt: false,
            runner: crate::templates_runner::TemplatesRunner::default(),
        };
        shell.palette.include_dev = options.demo;
        if options.demo {
            shell.load_demo();
        } else if shell.company_link.is_some() {
            // With an engine, start on the Open Company window (Q-182); the
            // remembered layout returns when the company opens.
            shell.dispatch_now(CommandId::new("file.open_company"));
        } else {
            shell.restore_layout_or_home();
        }
        shell
    }

    /// The engine link, when the shell has one.
    #[must_use]
    pub fn company_link(&self) -> Option<&Arc<CompanyLink>> {
        self.company_link.as_ref()
    }

    /// The open company (tenant id and name), if any.
    #[must_use]
    pub fn open_tenant(&self) -> Option<&(cl_core::ids::TenantId, String)> {
        self.open_tenant.as_ref()
    }

    /// Builds the screen for a window kind: a company screen when the
    /// kind is one (and the shell has a link), else a placeholder.
    fn make_screen(&self, kind: CommandId) -> Box<dyn Screen> {
        make_any(
            kind,
            self.company_link.as_ref(),
            self.open_tenant.clone(),
            self.today,
            self.prefs,
            self.demo,
        )
    }

    /// Bring the window for `kind` to the front, or open it.
    fn open_screen(&mut self, kind: CommandId) {
        // A finished interview (its "done" panel) is not reused: New Company
        // always starts fresh.
        if kind.as_str() == "file.new_company"
            && let Some(id) = self.windows.find_kind(kind)
            && self
                .windows
                .get(id)
                .is_some_and(|w| w.screen.state()["created"] == true)
        {
            self.windows.close_now(id);
        }
        if let Some(id) = self.windows.find_kind(kind) {
            self.windows.activate(id);
        } else {
            let screen = self.make_screen(kind);
            self.windows.open_new(kind, screen);
        }
    }

    /// Open a saved transaction in its own form (Q-P14g: a register row of
    /// a grouped deposit or a daily-sales day). A new window of `kind` loads
    /// it; a screen that shows no saved records just opens (or comes to the
    /// front) as usual.
    fn open_record(&mut self, kind: CommandId, txn: cl_core::ids::TransactionId) {
        let mut screen = self.make_screen(kind);
        if screen.open_record(txn) {
            self.windows.open_new(kind, screen);
        } else {
            self.open_screen(kind);
        }
    }

    /// Open a fresh form filled from a template (P15). A form that takes
    /// templates opens as a new window; any other kind just opens (or comes
    /// to the front) as usual.
    fn open_template(&mut self, template: crate::forms::template::TemplateOpen) {
        let kind = template.kind;
        let mut screen = self.make_screen(kind);
        if screen.open_template(template) {
            self.windows.open_new(kind, screen);
        } else {
            self.open_screen(kind);
        }
    }

    /// Open a fresh window of `kind` with what another screen handed over
    /// (P17). A screen that takes the payload opens as a new window; any
    /// other kind just opens (or comes to the front) as usual.
    fn open_with(&mut self, kind: CommandId, payload: crate::windows::OpenPayload) {
        let mut screen = self.make_screen(kind);
        if screen.open_with(payload) {
            self.windows.open_new(kind, screen);
        } else {
            self.open_screen(kind);
        }
    }

    /// Bring the register tab of `account` to the front, or open one (one
    /// tab per account, P12).
    fn open_register(&mut self, account: cl_core::ids::AccountId) {
        let kind = CommandId::new(crate::screens::register::KIND);
        let want = account.to_string();
        let found = self
            .windows
            .all()
            .iter()
            .find(|w| w.kind == kind && w.screen.state()["account_id"] == want.as_str())
            .map(|w| w.id);
        if let Some(id) = found {
            self.windows.activate(id);
            return;
        }
        let Some(link) = self.company_link.clone() else {
            return;
        };
        let screen = crate::screens::register::RegisterScreen::new(
            link,
            self.open_tenant.clone(),
            self.today,
            Some(account),
        );
        self.windows.open_new(kind, Box::new(screen));
    }

    /// Apply what the company screens reported since the last frame.
    fn apply_company_events(&mut self, ctx: Option<&Context>) {
        let Some(link) = self.company_link.clone() else {
            return;
        };
        link.set_dirty_windows(self.windows.dirty_ids().len());
        for event in link.drain() {
            match event {
                CompanyEvent::SignedIn { user } => {
                    self.status.user = Some(user);
                }
                CompanyEvent::Opened {
                    tenant_id,
                    name,
                    accent,
                    closed_through,
                } => {
                    let key = tenant_id.to_string();
                    // Close the previous company's windows (Q-182) before the
                    // new company's layout comes back; the interview stays so
                    // its "done" panel can offer the next steps.
                    self.save_layout();
                    let keep = CommandId::new("file.new_company");
                    for id in self
                        .windows
                        .all()
                        .iter()
                        .filter(|w| w.kind != keep)
                        .map(|w| w.id)
                        .collect::<Vec<_>>()
                    {
                        self.windows.close_now(id);
                    }
                    self.open_tenant = Some((tenant_id, name.clone()));
                    // The new company's Home reads its own counts.
                    self.counts = RailCounts::default();
                    self.set_company(
                        &key,
                        Some(CompanyBanner {
                            name,
                            closed_through,
                            accent: accent.0,
                        }),
                    );
                    if self.windows.is_empty() {
                        self.dispatch_now(CommandId::new("nav.home"));
                    }
                    // Catch-up runs after every open (P15, Q-P15h).
                    self.runner.opened(&link, tenant_id);
                    // This company's unknown saves (kept for the session).
                    link.lost_saves().set_company(Some(tenant_id));
                }
                CompanyEvent::Closed => {
                    self.runner.closed(&link);
                    link.lost_saves().set_company(None);
                    self.save_layout();
                    for id in self.windows.all().iter().map(|w| w.id).collect::<Vec<_>>() {
                        self.windows.close_now(id);
                    }
                    self.open_tenant = None;
                    self.company = None;
                    self.counts = RailCounts::default();
                    self.company_key = "default".into();
                    self.last_saved_layout = Some(self.windows.snapshot());
                    self.dispatch_now(CommandId::new("file.open_company"));
                }
                CompanyEvent::OpenWindow(id) => {
                    let kind = id;
                    self.open_screen(kind);
                }
                CompanyEvent::Toast(key) => {
                    if let Some(ctx) = ctx {
                        self.toast(ctx, key);
                    }
                }
                CompanyEvent::ClearToast(key) => {
                    if self.toast.as_ref().is_some_and(|t| t.key == key) {
                        self.toast = None;
                    }
                }
                CompanyEvent::UserPrefs(p) => {
                    self.prefs = p;
                    if let Some(store) = self.store.as_ref().filter(|_| !self.run_only)
                        && let Err(e) = store.save_prefs(&self.prefs)
                    {
                        tracing::warn!(error = %e, "could not save preferences");
                    }
                }
                CompanyEvent::PromptDirty => {
                    if let Some(id) = self.windows.dirty_ids().first().copied() {
                        self.return_after_prompt = self.windows.active_id();
                        self.windows.request_close(id);
                    }
                }
                CompanyEvent::CloseWindow(kind) => {
                    if let Some(id) = self.windows.find_kind(kind) {
                        self.windows.close_now(id);
                    }
                }
                CompanyEvent::OpenRegister(account) => self.open_register(account),
                CompanyEvent::OpenRecord(kind, txn) => self.open_record(kind, txn),
                CompanyEvent::OpenTemplate(t) => self.open_template(*t),
                CompanyEvent::OpenWith(kind, payload) => self.open_with(kind, *payload),
                CompanyEvent::RailCounts(c) => {
                    // Only while a company is open (Opened and Closed clear
                    // them; the new company's Home reads its own).
                    if self.open_tenant.is_some() || self.demo {
                        self.counts = c;
                    }
                }
            }
        }
    }

    /// Close the open company: save or prompt for open forms first, wait
    /// for pending writes, then return to the Open Company window.
    fn close_company(&mut self) {
        let Some(link) = self.company_link.clone() else {
            return;
        };
        if self.open_tenant.is_none() && self.company.is_none() {
            self.dispatch_now(CommandId::new("file.open_company"));
            return;
        }
        if !self.windows.dirty_ids().is_empty() {
            // The close prompt handles each dirty window first; the close
            // continues by itself after Save or Discard.
            if let Some(id) = self.windows.dirty_ids().first().copied() {
                self.close_company_after_prompt = true;
                self.windows.request_close(id);
            }
            return;
        }
        if link.pending_writes() > 0 {
            return;
        }
        if let Some(mut demo) = link.demo_state() {
            demo.open = None;
        } else if let Some(session) = link.session()
            && let Some(Ok(group)) = link.group()
        {
            let _ = group.read(&session, |s| async move { s.close_company().await });
        }
        link.push(CompanyEvent::Closed);
    }

    /// Keep theme and zoom changes for this run only: nothing is written to
    /// the preference file. Set while the control channel is on, so a script
    /// cannot change what the owner sees next time (P07 review L7).
    pub fn set_run_only(&mut self, on: bool) {
        self.run_only = on;
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
        // The rail counts come from Home's reads (P16), as in a real company.
        self.status = StatusInfo {
            user: Some("Kurt \u{b7} Admin".into()),
            mode: Some("Single-user \u{b7} this computer".into()),
            ai: Some("AI: Ollama, on this computer".into()),
        };
        if let Some(link) = &self.company_link
            && let Some(demo) = link.demo_state()
            && let Some(c) = demo.companies.first()
        {
            self.open_tenant = Some((c.tenant_id, c.name.clone()));
        }
        let mk = |id: &str| commands::find(id).map_or_else(|| CommandId::new("nav.home"), |c| c.id);
        let home = self.make_screen(mk("nav.home"));
        self.windows.open_new(mk("nav.home"), home);
        // The real Write Check with the mockup's unsaved draft (P13).
        let check: Box<dyn Screen> = match &self.company_link {
            Some(link) => Box::new(crate::screens::write_check::WriteCheckScreen::demo_draft(
                Arc::clone(link),
                self.open_tenant.clone(),
                self.today,
            )),
            None => Box::new(
                Placeholder::for_command(mk("vendors.write_check"))
                    .with_title("Write Check \u{b7} #4418")
                    .with_dirty(true),
            ),
        };
        self.windows.open_new(mk("vendors.write_check"), check);
        // The real register of the demo's operating account (P12).
        let register = self.make_screen(mk("banking.register"));
        self.windows.open_new(mk("banking.register"), register);
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
        let (link, open, today, prefs, demo) = (
            self.company_link.clone(),
            self.open_tenant.clone(),
            self.today,
            self.prefs,
            self.demo,
        );
        let restored = layout.is_some_and(|l| {
            self.windows.restore(&l, &|k| {
                make_any(k, link.as_ref(), open.clone(), today, prefs, demo)
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
        let (link, open, today, prefs, demo) = (
            self.company_link.clone(),
            self.open_tenant.clone(),
            self.today,
            self.prefs,
            self.demo,
        );
        let restored = layout.is_some_and(|l| {
            self.windows.restore(&l, &|k| {
                make_any(k, link.as_ref(), open.clone(), today, prefs, demo)
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
            key: key.to_owned(),
            text: t(key),
            until: now + 4.0,
        });
        ctx.request_repaint_after(Duration::from_secs(4));
    }

    /// The command with this id: the shipped table, plus the
    /// developer-only commands when the shell runs with `--demo` (S8).
    #[must_use]
    pub fn find_command(&self, id: &str) -> Option<&'static Command> {
        commands::find(id).or_else(|| {
            if self.demo {
                commands::find_dev(id)
            } else {
                None
            }
        })
    }

    /// Run a command now (outside a frame; tests and start-up).
    pub fn dispatch_now(&mut self, id: CommandId) {
        self.run(None, id);
        self.apply_company_events(None);
    }

    /// State of the window in front, for the control channel.
    #[must_use]
    pub fn active_screen_state(&self) -> serde_json::Value {
        self.windows
            .active()
            .map_or(serde_json::Value::Null, |w| w.screen.state())
    }

    fn run(&mut self, ctx: Option<&Context>, id: CommandId) {
        let target = self.windows.active_id();
        self.run_for(ctx, id, target);
    }

    /// Run a command for the window `target`: window and form commands go
    /// to that window (a popped-out window's own keys, S2), the rest are
    /// app-wide.
    fn run_for(&mut self, ctx: Option<&Context>, id: CommandId, target: Option<WindowId>) {
        let Some(cmd) = self.find_command(id.as_str()) else {
            return;
        };
        if cmd.opens {
            let kind = cmd.id;
            if self.company_link.is_some()
                && self.company.is_none()
                && kind.as_str() != "file.open_company"
                && kind.as_str() != "file.new_company"
            {
                // Nothing but Open Company and the interview works without a
                // company (Q-182); the restore panel lives in Open Company.
                let open = CommandId::new("file.open_company");
                self.open_screen(open);
                if let Some(ctx) = ctx {
                    self.toast(ctx, "toast.open_company_first");
                }
                return;
            }
            // Ctrl+G asks the window in front first: the Chart of Accounts
            // opens the selected account's register, a register stays.
            if kind.as_str() == crate::screens::register::KIND
                && let Some(w) = target.and_then(|id| self.windows.get_mut(id))
                && w.screen.command(kind)
            {
                return;
            }
            self.open_screen(kind);
            return;
        }
        match id.as_str() {
            "file.close_company" => self.close_company(),
            "file.restore" => {
                let open = CommandId::new("file.open_company");
                self.open_screen(open);
                let _ = self
                    .windows
                    .active_mut()
                    .is_some_and(|w| w.screen.command(CommandId::new("file.restore")));
            }
            "window.next" => self.windows.next(),
            "window.prev" => self.windows.prev(),
            "window.pop_out" => {
                if let Some(w) = target {
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
                if let Some(w) = target {
                    self.windows.request_close(w);
                }
            }
            "file.quit" => self.request_quit(ctx),
            "help.palette" => self.palette.toggle(),
            "help.shortcuts" => self.shortcuts_open = !self.shortcuts_open,
            "help.about" => self.about.open_about(),
            "help.privacy" => self.about.open_privacy(),
            // Only ever run from an explicit menu, palette or keyboard
            // choice; it opens the browser and makes no request itself.
            "help.kofi" => {
                if let Some(ctx) = ctx {
                    crate::links::open_external(ctx, crate::screens::home::support::KOFI_URL);
                }
            }
            "help.current" => {
                if let Some(ctx) = ctx {
                    self.toast(ctx, "toast.help_later");
                }
            }
            _ => self.forward_to_screen(ctx, cmd, target),
        }
    }

    /// Form and window commands go to the screen in front; commands nothing
    /// handles say so instead of silently doing nothing.
    fn forward_to_screen(
        &mut self,
        ctx: Option<&Context>,
        cmd: &Command,
        target: Option<WindowId>,
    ) {
        let handled = target
            .and_then(|id| self.windows.get_mut(id))
            .is_some_and(|w| w.screen.command(cmd.id));
        // Esc (form.revert): the screen reverts a field when it can; when
        // nothing is left to revert the window closes, asking first when
        // it still has unsaved edits (DESIGN.md form rules).
        if !handled
            && cmd.id.as_str() == "form.revert"
            && let Some(id) = target
        {
            self.windows.request_close(id);
            return;
        }
        // The plain Delete key reaches every window; only lists that delete
        // entries take it.
        let quiet = cmd.id.as_str().starts_with("form.")
            || cmd.id.as_str().starts_with("edit.")
            || cmd.id.as_str() == "list.delete";
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
        self.flush_widths(None);
        self.allow_close = true;
        if let Some(ctx) = ctx {
            // Quit may come from a popped-out window's keys: close the app.
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, ViewportCommand::Close);
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

    /// The window the main window's keys act on: the tab in front, unless
    /// it is popped out into its own operating-system window (that window
    /// handles its own keys).
    fn root_key_target(&self, ctx: &Context) -> Option<WindowId> {
        let active = self.windows.active()?;
        (!active.popped_out || ctx.embed_viewports()).then_some(active.id)
    }

    fn handle_keys(&mut self, ctx: &Context) {
        let target = self.root_key_target(ctx);
        self.handle_keys_for(ctx, target);
    }

    /// Resolve this viewport's shortcuts in `target`'s scope and run them
    /// for `target`. While a modal is open only the palette and the
    /// shortcut overlay keys work.
    fn handle_keys_for(&mut self, ctx: &Context, target: Option<WindowId>) {
        let pressed = ctx.input(input::pressed_shortcuts);
        if pressed.is_empty() {
            return;
        }
        let modal = self.modal_open();
        let window = target.and_then(|id| self.windows.get(id));
        let scope = window.map_or(Scope::Global, |w| w.screen.scope());
        // A focused text field keeps its edit keys; the window decides (a
        // grid lets Ctrl+A through when its cell has nothing to select).
        let kept: Vec<bool> = pressed
            .iter()
            .map(|s| {
                input::reserved_by_text_fields(*s)
                    && window.map_or_else(
                        || ctx.egui_wants_keyboard_input(),
                        |w| w.screen.keeps_text_key(ctx, *s),
                    )
            })
            .collect();
        let mut handled: Vec<cl_model::Shortcut> = Vec::new();
        for (s, kept) in pressed.into_iter().zip(kept) {
            let Some(id) = self.keymap.resolve(scope, s) else {
                continue;
            };
            if modal && !matches!(id.as_str(), "help.palette" | "help.shortcuts") {
                continue;
            }
            if kept {
                continue;
            }
            handled.push(s);
            self.run_for(Some(ctx), id, target);
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

    /// The Enter preference changed (Q-264): rebuild the keymap so
    /// Ctrl+Enter is Save & New only when Enter moves between fields.
    fn sync_keymap(&mut self) {
        let mode = EnterMode::from_pref(self.prefs.enter_moves_fields);
        if self.keymap.mode() == mode {
            return;
        }
        match Keymap::with_mode(COMMANDS, mode) {
            Ok(k) => self.keymap = k,
            // A unit test rules this out; keep the old keymap if it happens.
            Err(e) => tracing::warn!(error = %e, "could not rebuild the keymap"),
        }
    }

    /// What Enter does now (the preference, Q-264).
    #[must_use]
    pub fn enter_mode(&self) -> EnterMode {
        self.keymap.mode()
    }

    fn begin_frame(&mut self, ctx: &Context) {
        self.sync_keymap();
        if self.applied_theme != Some(self.prefs.theme) {
            theme::apply(ctx, self.prefs.theme);
            self.applied_theme = Some(self.prefs.theme);
        }
        if self.applied_zoom != Some(self.prefs.zoom_percent) {
            ctx.set_zoom_factor(self.prefs.zoom_factor());
            self.applied_zoom = Some(self.prefs.zoom_percent);
            if let Some(store) = self.store.as_ref().filter(|_| !self.run_only)
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

    /// The templates catch-up (P15): runs when a trigger fired; after a run
    /// the registers hear about the posts (Q-262) and Templates Due opens
    /// when there is something new to show.
    fn run_templates(&mut self, ctx: &Context) {
        let Some(link) = self.company_link.clone() else {
            return;
        };
        let now = ctx.input(|i| i.time);
        if let Some(done) = self.runner.tick(&link, self.today, now) {
            if done.posted {
                link.bus().publish(crate::data_bus::Change::Ledger(None));
            }
            if done.open_window {
                self.open_screen(CommandId::new(crate::screens::templates_due::KIND));
            }
        }
        if self.runner.busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else if self.runner.armed() {
            // The midnight and hourly triggers need a frame now and then.
            ctx.request_repaint_after(Duration::from_secs(60));
        }
    }

    /// The receipt inbox (P17): listed in the background while a company
    /// is open, often while the inbox or Enter Bill is in front; its count
    /// shows in the Vendors menu.
    fn tick_receipt_inbox(&self, ctx: &Context) {
        let Some(link) = &self.company_link else {
            return;
        };
        let fast = self
            .windows
            .active()
            .is_some_and(|w| crate::screens::receipt_inbox::FAST_KINDS.contains(&w.kind.as_str()));
        crate::screens::receipt_inbox::tick(
            ctx,
            link,
            self.open_tenant.as_ref().map(|t| t.0),
            self.today,
            fast,
        );
    }

    /// How many templates wait for the owner (the status-bar chip).
    #[must_use]
    pub fn templates_need_you(&self) -> usize {
        if self.open_tenant.is_none() {
            return 0;
        }
        self.company_link
            .as_ref()
            .map_or(0, |l| l.due().needs_you())
    }

    /// The text of the toast on screen, if any.
    #[must_use]
    pub fn toast_text(&self) -> Option<&str> {
        self.toast.as_ref().map(|t| t.text.as_str())
    }

    /// Draw one frame of the whole shell into the root `ui`.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        debug_assert!(
            cl_grid::sequencer::is_installed(&ctx),
            "install cl_grid::sequencer after Shell::install (and after the bridge)"
        );
        // Persist prefs changed by commands (theme).
        let before_theme = self.applied_theme;
        self.begin_frame(&ctx);
        if before_theme.is_some()
            && before_theme != self.applied_theme
            && !self.run_only
            && let Some(store) = &self.store
        {
            let _ = store.save_prefs(&self.prefs);
        }
        let narrow = ui.available_width() < NARROW_WIDTH;
        let tokens = Tokens::of(&ctx);

        self.apply_company_events(Some(&ctx));
        self.publish_grid_facts(&ctx, egui::ViewportId::ROOT);
        title_bar::show(ui, self, narrow, &tokens);
        menu::show(ui, self, narrow, &tokens);
        if status_bar::show(ui, self, narrow, &tokens) {
            self.open_screen(CommandId::new(crate::screens::templates_due::KIND));
        }
        // With no company open the Open Company window fills the body, as
        // in its mockup: no rail and no tab strip until a company opens.
        let chrome = self.company.is_some() || self.company_link.is_none();
        if chrome && (!narrow || self.rail_open_when_narrow) {
            rail::show(ui, self, &tokens);
        }
        if chrome {
            tabs::show(ui, self, narrow, &tokens);
        }
        self.body(ui, &tokens);
        self.show_popped_out(&ctx, &tokens);
        self.show_overlays(&ctx);

        for id in std::mem::take(&mut self.pending) {
            self.run(Some(&ctx), id);
        }
        self.apply_company_events(Some(&ctx));
        self.run_templates(&ctx);
        self.tick_receipt_inbox(&ctx);
        self.save_layout();
        let now = ctx.input(|i| i.time);
        self.flush_widths(Some(now));
        if self.widths.lock().is_ok_and(|w| {
            w.changed_at.is_some()
                || w.registers_changed_at.is_some()
                || w.panels_changed_at.is_some()
        }) {
            ctx.request_repaint_after(Duration::from_millis(1100));
        }
        if self.toast.is_some() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    /// Tell the grid screens of `viewport` today's date, the formats and
    /// whether a modal is open, and share the remembered column widths.
    fn publish_grid_facts(&self, ctx: &Context, viewport: egui::ViewportId) {
        let frame = crate::grid_env::ShellFrame::new(self.today, &self.prefs, self.modal_open());
        crate::grid_env::publish(ctx, viewport, frame);
        crate::grid_env::publish_widths(ctx, &self.widths);
        // Shift+Enter in a header field last frame: focus moves back now.
        crate::forms::enter::begin_frame(ctx);
    }

    /// Write changed column widths a second after the last change (`now`
    /// in egui seconds), or at once with `None` (quitting). Never during a
    /// controlled run.
    fn flush_widths(&mut self, now: Option<f64>) {
        let Some(store) = self.store.as_ref().filter(|_| !self.run_only) else {
            return;
        };
        let Ok(mut w) = self.widths.lock() else {
            return;
        };
        let due = w
            .changed_at
            .is_some_and(|at| now.is_none_or(|n| n - at >= 1.0));
        if due {
            w.changed_at = None;
            if let Err(e) = store.save_grid_widths(&w.book) {
                tracing::warn!(error = %e, "could not save column widths");
            }
        }
        let due = w
            .registers_changed_at
            .is_some_and(|at| now.is_none_or(|n| n - at >= 1.0));
        if due {
            w.registers_changed_at = None;
            if let Err(e) = store.save_register_prefs(&w.registers) {
                tracing::warn!(error = %e, "could not save register choices");
            }
        }
        let due = w
            .panels_changed_at
            .is_some_and(|at| now.is_none_or(|n| n - at >= 1.0));
        if due {
            w.panels_changed_at = None;
            if let Err(e) = w.panels.save(store.dir()) {
                tracing::warn!(error = %e, "could not save panel choices");
            }
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
        // With no company open the Open Company card sits on the app
        // background, as in its mockup.
        let fill = if self.company.is_none() && self.company_link.is_some() {
            tokens.surface_app
        } else {
            tokens.surface_window
        };
        let frame = Frame::new()
            .fill(fill)
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
                draw_screen(ui, w.screen.as_mut());
            }
        });
        if let Some(id) = dock {
            self.windows.dock(id);
        }
    }

    /// Windows moved out of the tab strip get their own operating-system
    /// window (egui viewport). Closing that window docks it back (Q-57).
    fn show_popped_out(&mut self, ctx: &Context, tokens: &Tokens) {
        let popped: Vec<(WindowId, String)> = self
            .windows
            .all()
            .iter()
            .filter(|w| w.popped_out)
            .map(|w| (w.id, w.title()))
            .collect();
        let mut dock: Vec<WindowId> = Vec::new();
        let fill = tokens.surface_window;
        for (wid, title) in popped {
            let id = egui::ViewportId::from_hash_of(("cl-window", wid.raw()));
            let builder = ViewportBuilder::default()
                .with_title(format!("{title} - ClientLedger"))
                .with_inner_size([900.0, 640.0])
                .with_min_inner_size(MIN_WINDOW);
            ctx.show_viewport_immediate(id, builder, |ui, class| {
                let vctx = ui.ctx().clone();
                if class == egui::ViewportClass::Immediate {
                    // Its own operating-system window: its keys and grid
                    // facts are its own (S2). Embedded windows share the
                    // main window's input, handled there.
                    self.publish_grid_facts(&vctx, vctx.viewport_id());
                    self.handle_keys_for(&vctx, Some(wid));
                }
                if let Some(w) = self.windows.get_mut(wid) {
                    let screen = &mut w.screen;
                    egui::CentralPanel::default()
                        .frame(Frame::new().fill(fill).inner_margin(Margin::same(16)))
                        .show(ui, |ui| draw_screen(ui, screen.as_mut()));
                }
                if vctx.input(|i| i.viewport().close_requested()) {
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
            self.prompt_focused = None;
            return;
        };
        // Focus Save on the frame the prompt opens, so typing lands on a
        // button (nothing happens) and Enter or Space saves (B6, T9).
        let focus_save = self.prompt_focused != Some(prompt);
        self.prompt_focused = Some(prompt);
        let title = self
            .windows
            .get(prompt.window)
            .map(crate::windows::Window::title)
            .unwrap_or_default();
        // A save whose outcome is unknown may already be in the books.
        let unconfirmed = self
            .windows
            .get(prompt.window)
            .is_some_and(crate::windows::Window::save_unconfirmed);
        let mut answer: Option<PromptAnswer> = None;
        egui::Window::new(t("window.prompt.title"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(if unconfirmed {
                    crate::windows::prompt_message_unconfirmed(&title)
                } else {
                    crate::windows::prompt_message(&title)
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let save = crate::widgets::primary_button(ui, &t("window.prompt.save"));
                    if focus_save {
                        save.request_focus();
                    }
                    if save.clicked() {
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
        if let Some(a) = answer {
            let progress = self.windows.answer_prompt(a);
            if a != PromptAnswer::Keep
                && let Some(back) = self.return_after_prompt.take()
            {
                self.windows.activate(back);
            }
            if a == PromptAnswer::Keep {
                self.return_after_prompt = None;
                self.close_company_after_prompt = false;
            } else if std::mem::take(&mut self.close_company_after_prompt) {
                self.close_company();
            }
            if progress == Some(QuitProgress::Ready) {
                self.finish_quit(Some(ctx));
            }
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

/// A company screen's constructor: the window for a command kind, or `None`
/// when the kind is another screen's.
pub(crate) type MakeScreen = fn(
    CommandId,
    &Arc<CompanyLink>,
    Option<(cl_core::ids::TenantId, String)>,
    CalendarDate,
) -> Option<Box<dyn Screen>>;

/// A screen for `kind` without borrowing the shell (layout restore). The
/// developer-only screens are built only with `--demo` (`demo`); every
/// path that opens a window (commands, company events, layout restore)
/// comes through here.
fn make_any(
    kind: CommandId,
    link: Option<&Arc<CompanyLink>>,
    open: Option<(cl_core::ids::TenantId, String)>,
    today: CalendarDate,
    prefs: UiPrefs,
    demo: bool,
) -> Box<dyn Screen> {
    if kind.as_str().starts_with("dev.") {
        if demo && let Some(s) = crate::screens::dev::make(kind, today, link) {
            return s;
        }
        return Box::new(Placeholder::for_command(kind));
    }
    if let Some(link) = link {
        if let Some(s) = crate::screens::company::make(kind, link, open.clone(), today) {
            return s;
        }
        if let Some(s) = crate::screens::lists::make(kind, link, open.clone(), today) {
            return s;
        }
        if let Some(s) = crate::screens::register::make(kind, link, open.clone(), today) {
            return s;
        }
        if let Some(s) = crate::screens::write_check::make(kind, link, open.clone(), today) {
            return s;
        }
        for make in crate::screens::P14_SCREENS
            .iter()
            .chain(&crate::screens::P15_SCREENS)
            .chain(&crate::screens::P15A_SCREENS)
            .chain(&crate::screens::P16_SCREENS)
            .chain(&crate::screens::P17_SCREENS)
        {
            if let Some(s) = make(kind, link, open.clone(), today) {
                return s;
            }
        }
        if let Some(s) = crate::screens::preferences::make(kind, link, open, prefs) {
            return s;
        }
    }
    Box::new(Placeholder::for_command(kind))
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
    cl_model::date_entry::format(d, cl_model::date_entry::DateOrder::Mdy)
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

/// A window body: a screen with fixed bars (Q-265) gets the rectangle and
/// scrolls its own body; any other screen scrolls whole.
fn draw_screen(ui: &mut egui::Ui, screen: &mut dyn crate::windows::Screen) {
    if screen.fixed_layout() {
        screen.ui(ui);
    } else {
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| screen.ui(ui));
    }
}

#[cfg(test)]
mod tests;
