//! A headless rig for Browse grids (registers) at any window size.

use cl_grid::{Grid, GridEnv, GridHeight, GridStyle, ModelRef, NoLookups, RowModel};
use egui::{Event, Pos2, RawInput, Rect, Vec2};

use super::lines::formats;
use super::rig::{context, text};

pub struct Browse<M: RowModel> {
    pub ctx: egui::Context,
    pub grid: Grid,
    pub model: M,
    pub style: GridStyle,
    pub last: Option<egui::FullOutput>,
    pub outer_offset: f32,
    pub outer: bool,
    pub size: Vec2,
    /// The last frame's grid events.
    pub events: Vec<cl_grid::GridEvent>,
}

impl<M: RowModel> Browse<M> {
    pub fn new(model: M, spec: cl_grid::GridSpec) -> Browse<M> {
        Browse {
            ctx: context(),
            grid: Grid::new("register", spec).expect("spec"),
            model,
            style: GridStyle::default(),
            last: None,
            outer_offset: 0.0,
            outer: false,
            size: Vec2::new(1440.0, 940.0),
            events: Vec::new(),
        }
    }

    pub fn frame(&mut self, events: Vec<Event>) -> usize {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            events,
            ..RawInput::default()
        };
        let style = self.style.clone();
        let (grid, model) = (&mut self.grid, &mut self.model);
        let outer = self.outer;
        let mut offset = 0.0;
        let mut events = Vec::new();
        let mut out = self.ctx.run_ui(input, |ui| {
            let env = GridEnv {
                style: &style,
                text: &text,
                lookups: &NoLookups,
                today: formats().today,
                first_weekday: formats().first_weekday,
                date_order: formats().date_order,
                money_style: formats().money,
                height: GridHeight::Fill {
                    reserve_below: 0.0,
                    min_rows: 3,
                },
                label: "Register",
                input_enabled: true,
            };
            if outer {
                let o = egui::ScrollArea::both().id_salt("outer").show(ui, |ui| {
                    ui.set_min_height(3_000.0);
                    let r = ui.allocate_ui(Vec2::new(1400.0, 600.0), |ui| {
                        events = grid.show(ui, ModelRef::Browse(model), &env).events;
                    });
                    let _ = r;
                });
                offset = o.state.clamped_offset().y;
            } else {
                events = grid.show(ui, ModelRef::Browse(model), &env).events;
            }
        });
        out.textures_delta.clear();
        self.outer_offset = offset;
        self.events = events;
        let shapes = out.shapes.len();
        self.last = Some(out);
        shapes
    }

    pub fn node_bounds(&self, id: egui::Id) -> Option<egui::accesskit::Rect> {
        let nodes = &self
            .last
            .as_ref()?
            .platform_output
            .accesskit_update
            .as_ref()?
            .nodes;
        nodes
            .iter()
            .find(|(n, _)| *n == id.accesskit_id())
            .and_then(|(_, n)| n.bounds())
    }

    pub fn focus(&mut self) {
        let id = self.grid.focus_id();
        self.ctx.memory_mut(|m| m.request_focus(id));
        self.frame(vec![]);
        self.frame(vec![]);
        self.frame(vec![]);
    }
}
