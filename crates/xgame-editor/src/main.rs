//! XGameEngine 编辑器——类 Unity 编辑器界面，基于 egui_tiles。
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
mod scene_renderer;

use panels::state::EditorState;

struct EditorApp {
    tree: egui_tiles::Tree<panels::dock::PanelTab>,
    state: EditorState,
}

impl EditorApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut state = EditorState::new();

        // Initialise the 3D scene renderer from the wgpu device that eframe
        // already created for the egui-wgpu integration.
        state.scene_renderer = cc.wgpu_render_state.as_ref().map(|rs| {
            scene_renderer::SceneRenderer::new(
                &rs.device,
                &rs.queue,
                rs.renderer.clone(),
                rs.target_format,
            )
        });

        Self {
            tree: panels::dock::create_tile_tree(),
            state,
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
        // Scene View panel triggers 3D rendering only when it is visible.
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
        Box::new(|cc| Ok(Box::new(EditorApp::new(cc)))),
    )
}
