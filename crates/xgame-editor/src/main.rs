//! XGameEngine Editor — Unity-inspired editor interface.
//!
//! Layout (from top to bottom):
//!
//! ┌──────────────────────────────────────────────────────────┐
//! │ [Toolbar: ▶ Play | ⏸ Pause | ⏹ Stop | ↔↻⇔▭ | Grid]    │
//! ├────────────┬──────────────────────────────┬──────────────┤
//! │            │                              │              │
//! │ Hierarchy  │     Scene View (3D)          │  Inspector   │
//! │            │                              │  (selected   │
//! │ Entity 1   │  ┌──────────────────────┐   │   entity)    │
//! │ Entity 2   │  │  ╔══╗  grid  / gizmo │   │  ─────────   │
//! │            │  │  ╚══╝                │   │  Position     │
//! │            │  └──────────────────────┘   │  Rotation     │
//! │            │                              │  Scale        │
//! ├────────────┴──────────────────────────────┴──────────────┤
//! │ Console (log messages with severity colors)              │
//! ├──────────────────────────────────────────────────────────┤
//! │ Status Bar: Editor Framework v0.1.0                      │
//! └──────────────────────────────────────────────────────────┘
//!
//! Run with:  cargo run -p xgame-editor

mod panels;
use panels::state::EditorState;

struct EditorApp {
    state: EditorState,
}

impl EditorApp {
    fn new() -> Self {
        Self {
            state: EditorState::new(),
        }
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ── 0. Apply Unity dark theme ──
        panels::theme::install(ctx);

        // ── 1. Toolbar (top) ──
        panels::toolbar::show(&mut self.state, ctx);

        // ── 2. Hierarchy panel (left) ──
        panels::hierarchy::show(&mut self.state, ctx);

        // ── 3. Inspector panel (right) ──
        panels::inspector::show(&mut self.state, ctx);

        // ── 4. Scene view (center) ──
        panels::scene_view::show(&mut self.state, ctx);

        // ── 5. Console panel (bottom) ──
        panels::console::show(&mut self.state, ctx);

        // ── 6. Status bar (bottom-most) ──
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Editor Framework v0.1.0");
                ui.separator();
                let btn_text = match self.state.play_mode {
                    panels::state::PlayMode::Edit => "Edit Mode",
                    panels::state::PlayMode::Playing => "Playing",
                    panels::state::PlayMode::Paused => "Paused",
                };
                ui.label(btn_text);
                ui.separator();
                ui.label(format!(
                    "Scene: {} | {} entities",
                    self.state.scene.name,
                    self.state.scene.entities.len(),
                ));
                ui.separator();
                ui.label(format!(
                    "FPS: {:.0}",
                    ctx.input(|i| i.predicted_dt).recip().round(),
                ));
            });
        });
    }
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_title("XGameEngine Editor"),
        ..Default::default()
    };

    eframe::run_native(
        "XGameEngine Editor",
        options,
        Box::new(|_cc| Ok(Box::new(EditorApp::new()))),
    )
}
