//! XGameEngine Editor — Unity-inspired editor interface with egui_tiles.
//!
//! Layout:
//!
//! ┌──────────────────────────────────────────────────────────┐
//! │ [Toolbar: ▶ Play | ⏸ Pause | ⏹ Stop | ↔↻⇔▭ | Grid]    │
//! ├────────────┬───────────────────────┬────────────────────┤
//! │ Hierarchy  │  Scene View           │  Inspector         │
//! │  (tab)     │  (tab)                │  (tab)             │
//! │            ├───────────────────────┤  egui_tiles        │
//! │            │  Console (tab)        │  manages splits    │
//! ├────────────┴───────────────────────┴────────────────────┤
//! │ Status Bar: v0.1.0 | Edit Mode | Scene: Untitled...     │
//! └──────────────────────────────────────────────────────────┘
//!
//! Run with:  cargo run -p xgame-editor

mod panels;

use panels::state::EditorState;

struct EditorApp {
    tree: egui_tiles::Tree<panels::dock::PanelTab>,
    state: EditorState,
}

impl EditorApp {
    fn new() -> Self {
        Self {
            tree: panels::dock::create_tile_tree(),
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

        // ── 2. Central dock area (tile tree with all panels) ──
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut behavior = panels::dock::EditorBehavior {
                state: &mut self.state,
            };
            self.tree.ui(&mut behavior, ui);
        });

        // ── 3. Status bar (bottom) ──
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
