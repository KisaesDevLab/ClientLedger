//! A headless egui rig: Plex fonts with `MissingGlyphPolicy::Panic`, the
//! input sequencer, a header field before the grid and a footer field after
//! it, and frames fed back to back with no idle frames in between.

use std::collections::BTreeMap;
use std::sync::Arc;

use cl_grid::{Grid, GridEnv, GridHeight, GridOutput, GridSpec, GridStyle, ModelRef, TextKey};
use egui::{
    Context, Event, FontData, FontDefinitions, FontFamily, Id, Key, MissingGlyphPolicy, Modifiers,
    Pos2, RawInput, Rect, Vec2,
};

use super::lines::{Lines, TestLookups, formats};

const SANS: &[u8] = include_bytes!("../../../../assets/fonts/IBMPlexSans-Regular.ttf");
const MONO: &[u8] = include_bytes!("../../../../assets/fonts/IBMPlexMono-Regular.ttf");

/// English for the grid's own keys; other keys show as themselves.
pub fn text(k: TextKey) -> String {
    match k.0 {
        "grid.a11y.cell" => "{column}, {row}".into(),
        "grid.a11y.line" => "line {n}".into(),
        "grid.a11y.row_of" => "row {n} of {total}".into(),
        "grid.calc.preview" => "= {value}".into(),
        "grid.popup.add_new" => "Add \"{text}\"".into(),
        other => other.into(),
    }
}

pub fn context() -> Context {
    let ctx = Context::default();
    let mut font_data = BTreeMap::new();
    font_data.insert("sans".to_string(), Arc::new(FontData::from_static(SANS)));
    font_data.insert("mono".to_string(), Arc::new(FontData::from_static(MONO)));
    let mut families = BTreeMap::new();
    families.insert(FontFamily::Proportional, vec!["sans".to_string()]);
    families.insert(FontFamily::Monospace, vec!["mono".to_string()]);
    ctx.set_fonts(FontDefinitions {
        font_data,
        families,
    });
    ctx.set_missing_glyph_policy(MissingGlyphPolicy::Panic);
    ctx.enable_accesskit();
    cl_grid::sequencer::install(&ctx);
    ctx
}

pub fn key(k: Key, m: Modifiers) -> Vec<Event> {
    vec![
        Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: m,
        },
        Event::Key {
            key: k,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: m,
        },
    ]
}

pub fn tab() -> Vec<Event> {
    key(Key::Tab, Modifiers::NONE)
}

pub fn shift_tab() -> Vec<Event> {
    key(Key::Tab, Modifiers::SHIFT)
}

pub fn typed(s: &str) -> Vec<Event> {
    vec![Event::Text(s.into())]
}

pub struct Rig {
    pub ctx: Context,
    pub grid: Grid,
    pub model: Lines,
    pub lookups: TestLookups,
    pub style: GridStyle,
    pub header: String,
    pub footer: String,
    pub size: Vec2,
    pub last: Option<egui::FullOutput>,
}

pub const HEADER: &str = "header-field";
pub const FOOTER: &str = "footer-field";

impl Rig {
    pub fn new(spec: GridSpec, model: Lines) -> Rig {
        Rig {
            ctx: context(),
            grid: Grid::new("test", spec).expect("spec"),
            model,
            lookups: TestLookups::default(),
            style: GridStyle::default(),
            header: String::new(),
            footer: String::new(),
            size: Vec2::new(1000.0, 600.0),
            last: None,
        }
    }

    pub fn header_id() -> Id {
        Id::unique(HEADER)
    }

    pub fn footer_id() -> Id {
        Id::unique(FOOTER)
    }

    /// One frame with these events.
    pub fn frame(&mut self, events: Vec<Event>) -> GridOutput {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            events,
            ..RawInput::default()
        };
        let mut result = GridOutput::default();
        let style = self.style.clone();
        let lookups = &self.lookups;
        let grid = &mut self.grid;
        let model = &mut self.model;
        let header = &mut self.header;
        let footer = &mut self.footer;
        let mut out = self.ctx.run_ui(input, |ui| {
            ui.add(egui::TextEdit::singleline(header).id(Rig::header_id()));
            let env = GridEnv {
                style: &style,
                text: &text,
                lookups,
                today: formats().today,
                first_weekday: formats().first_weekday,
                date_order: formats().date_order,
                money_style: formats().money,
                height: GridHeight::Rows { min: 3, max: 12 },
                label: "Expense lines",
                input_enabled: true,
            };
            result = grid.show(ui, ModelRef::Edit(model), &env);
            ui.add(egui::TextEdit::singleline(footer).id(Rig::footer_id()));
        });
        out.textures_delta.clear();
        self.last = Some(out);
        result
    }

    /// Frames until the sequencer has delivered everything.
    pub fn idle(&mut self) {
        for _ in 0..20 {
            self.frame(vec![]);
            if !cl_grid::sequencer::pending(&self.ctx, egui::ViewportId::ROOT) {
                break;
            }
        }
    }

    pub fn focused(&self) -> Option<Id> {
        self.ctx.memory(egui::Memory::focused)
    }

    pub fn focus_header(&mut self) {
        self.ctx.memory_mut(|m| m.request_focus(Rig::header_id()));
        self.frame(vec![]);
        self.frame(vec![]);
    }

    /// Put the grid's cursor on a cell and give it focus.
    pub fn focus_cell(&mut self, row: usize, col: cl_grid::ColumnId) {
        let pos = cl_grid::CellPos {
            row: self.model.key(row),
            col,
        };
        let style = self.style.clone();
        let env = GridEnv {
            style: &style,
            text: &text,
            lookups: &self.lookups,
            today: formats().today,
            first_weekday: formats().first_weekday,
            date_order: formats().date_order,
            money_style: formats().money,
            height: GridHeight::Rows { min: 3, max: 12 },
            label: "Expense lines",
            input_enabled: true,
        };
        self.grid.focus(pos, ModelRef::Edit(&mut self.model), &env);
        self.frame(vec![]);
        self.frame(vec![]);
    }

    /// Run `f` with the grid, the model and a frame-free env (commands and
    /// saves arrive between frames, as from `Screen::command`).
    pub fn with_env<R>(&mut self, f: impl FnOnce(&mut Grid, ModelRef<'_>, &GridEnv<'_>) -> R) -> R {
        let style = self.style.clone();
        let env = GridEnv {
            style: &style,
            text: &text,
            lookups: &self.lookups,
            today: formats().today,
            first_weekday: formats().first_weekday,
            date_order: formats().date_order,
            money_style: formats().money,
            height: GridHeight::Rows { min: 3, max: 12 },
            label: "Expense lines",
            input_enabled: true,
        };
        f(&mut self.grid, ModelRef::Edit(&mut self.model), &env)
    }

    /// A forwarded command, then one frame.
    pub fn command(&mut self, cmd: cl_grid::GridCommand) -> cl_grid::Handled {
        let (h, _) = self.with_env(|g, m, env| g.command(cmd, m, env));
        self.frame(vec![]);
        h
    }

    /// `Grid::commit_pending` (a save path).
    pub fn commit_pending(&mut self) -> Result<(), cl_grid::CellProblem> {
        self.with_env(Grid::commit_pending)
    }

    pub fn at(&self) -> (usize, cl_grid::ColumnId) {
        let c = self.grid.core().cursor().expect("cursor");
        (
            cl_grid::RowModel::index_of(&self.model, c.row).expect("row"),
            c.col,
        )
    }

    /// A node's bounds in the last frame, in points.
    // AccessKit bounds are f64 copies of egui's f32 points.
    #[allow(clippy::cast_possible_truncation)]
    pub fn bounds(&self, id: Id) -> Option<Rect> {
        self.nodes()
            .into_iter()
            .find(|(n, _)| *n == id.accesskit_id())
            .and_then(|(_, n)| n.bounds())
            .map(|r| {
                Rect::from_min_max(
                    Pos2::new(r.x0 as f32, r.y0 as f32),
                    Pos2::new(r.x1 as f32, r.y1 as f32),
                )
            })
    }

    /// The node with this id in the last frame.
    pub fn node(&self, id: Id) -> Option<egui::accesskit::Node> {
        self.nodes()
            .into_iter()
            .find(|(n, _)| *n == id.accesskit_id())
            .map(|(_, n)| n)
    }

    /// A primary click at `p`: press in one frame, release in the next.
    pub fn click_at(&mut self, p: Pos2) -> Vec<cl_grid::GridEvent> {
        let mut events = self
            .frame(vec![
                Event::PointerMoved(p),
                Event::PointerButton {
                    pos: p,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ])
            .events;
        events.extend(
            self.frame(vec![Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }])
            .events,
        );
        events
    }

    /// The AccessKit nodes of the last frame.
    pub fn nodes(&self) -> Vec<(egui::accesskit::NodeId, egui::accesskit::Node)> {
        self.last
            .as_ref()
            .and_then(|o| o.platform_output.accesskit_update.as_ref())
            .map(|u| u.nodes.clone())
            .unwrap_or_default()
    }
}
