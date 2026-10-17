//! The eframe application: window options, the app object, and the focus and
//! stop requests a second launch can send (Q-P04c).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;

use crate::automation::Bridge;
use crate::shell::{MIN_WINDOW, Shell, ShellOptions};

/// What the binary hands over at start-up.
pub struct LaunchOptions {
    /// Shell construction options.
    pub shell: ShellOptions,
    /// Set by another thread (the single-instance listener) to bring the
    /// window forward.
    pub focus_request: Option<Arc<AtomicBool>>,
    /// Set by another thread to ask for a clean exit (`host stop`).
    pub stop_request: Option<Arc<AtomicBool>>,
    /// Initial window size in points (before zoom).
    pub inner_size: [f32; 2],
    /// Keep the engine alive for the app's life: dropped on exit, which stops
    /// it.
    pub keep_alive: Option<Box<dyn std::any::Any + Send>>,
    /// The control channel, present only when the app was started with
    /// `--control` (P07).
    pub control: Option<cl_automation::ControlServer>,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        LaunchOptions {
            shell: ShellOptions::default(),
            focus_request: None,
            stop_request: None,
            inner_size: [1280.0, 820.0],
            keep_alive: None,
            control: None,
        }
    }
}

/// The window the OS shows: title, size limits, icon.
#[must_use]
pub fn native_options(inner_size: [f32; 2]) -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(crate::i18n::t("app.name"))
        .with_inner_size(inner_size)
        .with_min_inner_size(MIN_WINDOW);
    if let Some(icon) = crate::icon::viewport_icon() {
        viewport = viewport.with_icon(icon);
    }
    eframe::NativeOptions {
        viewport,
        ..eframe::NativeOptions::default()
    }
}

/// The app object eframe drives.
pub struct ClientLedgerApp {
    shell: Shell,
    focus_request: Option<Arc<AtomicBool>>,
    stop_request: Option<Arc<AtomicBool>>,
    bridge: Option<Bridge>,
    _keep_alive: Option<Box<dyn std::any::Any + Send>>,
}

impl ClientLedgerApp {
    /// Build the app inside eframe's creation callback.
    #[must_use]
    pub fn new(cc: &eframe::CreationContext<'_>, options: LaunchOptions) -> ClientLedgerApp {
        Shell::install(&cc.egui_ctx);
        ClientLedgerApp {
            shell: Shell::new(options.shell),
            focus_request: options.focus_request,
            stop_request: options.stop_request,
            bridge: options
                .control
                .map(|server| Bridge::install(&cc.egui_ctx, server)),
            _keep_alive: options.keep_alive,
        }
    }

    /// The shell (tests and the automation layer).
    pub fn shell_mut(&mut self) -> &mut Shell {
        &mut self.shell
    }
}

impl eframe::App for ClientLedgerApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(bridge) = &mut self.bridge {
            bridge.poll(ctx, &mut self.shell);
        }
        if self
            .focus_request
            .as_ref()
            .is_some_and(|f| f.swap(false, Ordering::SeqCst))
        {
            Shell::focus_app(ctx);
        }
        if self
            .stop_request
            .as_ref()
            .is_some_and(|f| f.swap(false, Ordering::SeqCst))
        {
            self.shell
                .dispatch_now(cl_model::CommandId::new("file.quit"));
            if self.shell.quit_approved() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.shell.ui(ui);
        // A second launch or `host stop` arrives on another thread; wake up
        // at least twice a second to notice it.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
    }

    fn on_exit(&mut self) {
        self.shell.save_layout();
    }
}

/// Run the app until the window closes.
///
/// # Errors
/// Any error eframe reports while starting the window.
pub fn run(options: LaunchOptions) -> eframe::Result {
    let native = native_options(options.inner_size);
    let name = crate::i18n::t("app.name");
    eframe::run_native(
        &name,
        native,
        Box::new(move |cc| Ok(Box::new(ClientLedgerApp::new(cc, options)))),
    )
}
