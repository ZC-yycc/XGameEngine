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
mod game_runtime;
mod scene_renderer;
mod gizmo;

use panels::state::{EditorState, PlayMode};

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
        // ── Keyboard state is read inline via ui.input(|i| i.key_down(...))
        // in scene_view.rs — no manual pressed_keys tracking needed.

        // ── 1. Apply Unity dark theme ──
        panels::theme::install(ctx);

        // ── 2. Detect play-mode transitions (toolbar may change play_mode) ──
        let prev_mode = self.state.play_mode;
        panels::toolbar::show(&mut self.state, ctx);
        match (prev_mode, self.state.play_mode) {
            (PlayMode::Edit, PlayMode::Playing) | (PlayMode::Paused, PlayMode::Playing) => {
                if self.state.game_runtime.is_none() {
                    self.state.enter_play_mode();
                }
            }
            (PlayMode::Playing, PlayMode::Edit) | (PlayMode::Paused, PlayMode::Edit) => {
                self.state.exit_play_mode();
            }
            _ => {}
        }

        // ── 3. Physics step (only when actively playing) ──
        if self.state.play_mode == PlayMode::Playing {
            let dt = ctx.input(|i| i.predicted_dt);
            self.state.physics_step(dt);
            // Force continuous repaint during play — eframe only repaints
            // on input events by default, which would freeze physics.
            ctx.request_repaint();
        }

        // ── 4. Central dock area (tile tree with all panels) ──
        // Scene View panel triggers 3D rendering only when it is visible.
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut behavior = panels::dock::EditorBehavior {
                state: &mut self.state,
            };
            self.tree.ui(&mut behavior, ui);
        });

        // ── 5. Status bar (bottom) ──
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Editor Framework v0.1.0");
                ui.separator();

                // Play mode indicator
                let btn_text = match self.state.play_mode {
                    panels::state::PlayMode::Edit => "Edit Mode",
                    panels::state::PlayMode::Playing => "▶ Playing",
                    panels::state::PlayMode::Paused => "⏸ Paused",
                };
                ui.label(btn_text);
                ui.separator();

                // Scene info
                ui.label(format!(
                    "Scene: {} | {} entities",
                    self.state.scene.name,
                    self.state.scene.entities.len(),
                ));
                ui.separator();

                // Physics stats (only when playing)
                if self.state.play_mode != PlayMode::Edit {
                    let (bodies, colliders) = self.state.physics_stats();
                    ui.label(format!("Bodies: {} | Colliders: {}", bodies, colliders));
                    ui.separator();
                }

                // FPS
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
