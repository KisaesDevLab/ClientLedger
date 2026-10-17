//! Developer aid: open the demo shell, wait a few frames, save a screenshot as
//! raw RGBA (`<width> <height>\n` then the pixels) and exit. The control
//! channel and `cargo xtask ui-check` (P07) replace this; until then it lets a
//! reviewer see the shell next to its mockup.
//!
//! `cargo run -p cl-ui --example shell_shot -- <out.rgba> [light|dark] [zoom%] [WxH] [keys]`
//! where `keys` is a comma list of command ids to run first, for example
//! `help.palette`.

use cl_model::CommandId;
use cl_ui::shell::{Shell, ShellOptions};
use cl_ui::theme::ThemeChoice;
use eframe::egui;

struct Shot {
    shell: Shell,
    out: std::path::PathBuf,
    frame: u32,
    pending: Vec<String>,
    tx: std::sync::mpsc::Sender<std::sync::Arc<egui::ColorImage>>,
    rx: std::sync::mpsc::Receiver<std::sync::Arc<egui::ColorImage>>,
}

impl eframe::App for Shot {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.shell.ui(ui);
        self.frame += 1;
        if self.frame == 3 {
            for id in self.pending.drain(..) {
                // Command ids are 'static in the table; find the registered one.
                if let Some(c) = cl_model::commands::find(&id) {
                    self.shell.dispatch_now(CommandId::new(c.id.as_str()));
                }
            }
        }
        if self.frame == 8 {
            let tx = self.tx.clone();
            ui.ctx().request_screenshot(move |image| {
                let _ = tx.send(image);
            });
        }
        if let Ok(image) = self.rx.try_recv() {
            let [w, h] = image.size;
            let mut bytes = format!("{w} {h}\n").into_bytes();
            for p in &image.pixels {
                bytes.extend_from_slice(&p.to_array());
            }
            let _ = std::fs::write(&self.out, bytes);
            // The demo has unsaved changes, so a normal close would prompt.
            std::process::exit(0);
        }
        ui.ctx().request_repaint();
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(args.first().cloned().unwrap_or_else(|| "shot.rgba".into()));
    let theme = match args.get(1).map(String::as_str) {
        Some("dark") => ThemeChoice::Dark,
        _ => ThemeChoice::Light,
    };
    let zoom = args.get(2).and_then(|z| z.parse::<u16>().ok());
    let size = args
        .get(3)
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some([w.parse::<f32>().ok()?, h.parse::<f32>().ok()?]))
        .unwrap_or([1440.0, 940.0]);
    let pending: Vec<String> = args
        .get(4)
        .map(|k| k.split(',').map(str::to_string).collect())
        .unwrap_or_default();
    eframe::run_native(
        "ClientLedger shot",
        cl_ui::app::native_options(size),
        Box::new(move |cc| {
            Shell::install(&cc.egui_ctx);
            let (tx, rx) = std::sync::mpsc::channel();
            Ok(Box::new(Shot {
                shell: Shell::new(ShellOptions {
                    demo: true,
                    theme: Some(theme),
                    zoom_percent: zoom,
                    ..ShellOptions::default()
                }),
                out,
                frame: 0,
                pending,
                tx,
                rx,
            }))
        }),
    )
}
