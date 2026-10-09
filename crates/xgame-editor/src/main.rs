//! XGameEngine 编辑器——类 Unity 编辑器界面，基于 egui_tiles。
//!
//! # 两个界面
//!
//! 编辑器二进制承担两个界面，靠 [`EditorApp::session`] 的有无切换：
//!
//! ```text
//! ┌──────────────────────────┐   选中项目    ┌──────────────────────────┐
//! │ 项目枢纽 ProjectHub       │ ───────────▶ │ 编辑器 EditorSession      │
//! │  · 新建项目 / 打开项目     │              │  · Hierarchy / Scene View │
//! │  · 最近项目列表           │ ◀─────────── │  · Inspector / Console    │
//! └──────────────────────────┘  「关闭项目」  └──────────────────────────┘
//! ```
//!
//! # 布局（编辑器界面）
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────┐
//! │ [⌂ Projects | ▶ Play | ⏸ Pause | ⏹ Stop | ↔↻⇔▭ | Grid] │
//! ├────────────┬───────────────────────┬────────────────────┤
//! │ Hierarchy  │  Scene View           │  Inspector         │
//! │  (tab)     │  (tab)                │  (tab)             │
//! │            ├───────────────────────┤  egui_tiles        │
//! │            │  Console (tab)        │  manages splits    │
//! ├────────────┴───────────────────────┴────────────────────┤
//! │ Status Bar: v0.1.0 | Project: xxx | Edit Mode | ...     │
//! └──────────────────────────────────────────────────────────┘
//! ```
//!
//! # 运行
//!
//! ```sh
//! # 打开项目枢纽（交互式选择项目）
//! cargo run -p xgame-editor
//!
//! # 直接打开指定项目
//! cargo run -p xgame-editor -- projects/SampleScene
//! ```

mod builder;
mod editor_input;
mod game_runtime;
mod gizmo;
mod input_bridge;
mod hub;
mod panels;
mod recents;
mod scene_renderer;
mod state;

use std::path::Path;
use std::sync::Arc;

use hub::{HubAction, ProjectHub};
use panels::dock::PanelTab;
use state::{EditorState, PlayMode};

// ---------------------------------------------------------------------------
// 应用状态
// ---------------------------------------------------------------------------

/// eframe 提供的 wgpu 资源。
///
/// 编辑器启动时停在项目枢纽，此时还不需要 `SceneRenderer`；但设备 / 队列 / egui
/// 渲染器要先留着——打开项目之后才用它们构造离屏渲染器。
struct GpuHandles {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    egui_renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
    target_format: wgpu::TextureFormat,
}

/// 打开项目之后进入的编辑器会话。
struct EditorSession {
    tree: egui_tiles::Tree<PanelTab>,
    state: EditorState,
    /// 上一次发给操作系统的窗口标题。
    ///
    /// `ViewportCommand::Title` 最终落到 winit 的 `SetWindowText`，是一条真实的
    /// 系统调用。每帧无脑重发会让窗口每帧被改一次标题，是肉眼可见的卡顿源——
    /// 所以缓存下来，只在项目名真的变了（即刚打开另一个项目）时才发。
    window_title: String,
}

struct EditorApp {
    /// 项目枢纽（没有打开项目时显示）。
    hub: ProjectHub,
    /// 当前编辑器会话；`None` 表示停留在项目枢纽。
    session: Option<EditorSession>,
    /// wgpu 资源句柄（wgpu 后端不可用时为 `None`）。
    gpu: Option<GpuHandles>,
}

impl EditorApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // ── 字体：赶在第一帧之前装好，否则项目枢纽第一屏就是一片方块字 ──
        // 只装这一次 —— `set_fonts` 会重建整张字形图集，不能每帧来。
        // （主题不在这里设：它必须**每帧**重设，见 `EditorApp::update`。）
        panels::theme::install_fonts(&cc.egui_ctx);

        let gpu = cc.wgpu_render_state.as_ref().map(|rs| GpuHandles {
            device: rs.device.clone(),
            queue: rs.queue.clone(),
            egui_renderer: rs.renderer.clone(),
            target_format: rs.target_format,
        });

        let mut app = Self {
            hub: ProjectHub::new(),
            session: None,
            gpu,
        };

        // 命令行参数：`xgame-editor <项目路径>` 直接进编辑器，跳过枢纽。
        // 路径无效时不退出——错误显示在枢纽界面上，用户还能另选一个。
        if let Some(arg) = std::env::args().nth(1) {
            // 先取出返回值，让 `&mut app.hub` 的借用在这里结束，
            // 否则下面 `app.open_project` 会与它冲突。
            let action = app.hub.try_open(Path::new(&arg));
            if let HubAction::Open(project) = action {
                app.open_project(*project);
            }
        }

        app
    }

    /// 打开项目：构造编辑器状态 + 离屏渲染器，进入编辑器界面。
    fn open_project(&mut self, project: xgame_core::ProjectHandle) {
        let mut state = EditorState::new(project);

        if let Some(gpu) = &self.gpu {
            let project_root = state.project.root().to_path_buf();
            state.scene_renderer = Some(scene_renderer::SceneRenderer::new(
                &gpu.device,
                &gpu.queue,
                gpu.egui_renderer.clone(),
                gpu.target_format,
                project_root,
            ));
        }

        self.session = Some(EditorSession {
            tree: panels::dock::create_tile_tree(),
            state,
            // 空串保证进入编辑器后的第一帧一定会把标题刷成项目名。
            window_title: String::new(),
        });
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ── 主题：每帧重设，别挪走 ──
        // eframe 每帧会按**系统主题**（这台机器是浅色）覆盖 egui 的 style，
        // 只在启动时设一次的话，界面会被系统浅色主题顶掉 —— 枢纽和编辑器界面都一样。
        // `set_style` 只是赋一个 Arc，开销可以忽略；真正重的是 `set_fonts`，
        // 那个在 `EditorApp::new` 里只做一次（重建字形图集很贵）。
        panels::theme::install(ctx);

        // ── 编辑器界面 ──
        let mut close_requested = false;
        if let Some(session) = self.session.as_mut() {
            editor_ui(session, ctx);
            if session.state.return_to_hub {
                session.state.return_to_hub = false;
                close_requested = true;
            }
        }

        if close_requested {
            self.session = None;
        }

        // ── 项目枢纽 ──
        if self.session.is_none() {
            let action = self.hub.ui(ctx);
            if let HubAction::Open(project) = action {
                self.open_project(*project);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 编辑器界面
// ---------------------------------------------------------------------------

/// 绘制编辑器主界面（工具栏 → 物理步进 → 停靠面板 → 状态栏）。
fn editor_ui(session: &mut EditorSession, ctx: &egui::Context) {
    // ── 1. 窗口标题带上项目名（仅变化时发，见 EditorSession::window_title）──
    let project_name = session.state.project_name();
    if session.window_title != project_name {
        session.window_title = project_name.to_owned();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "XGameEngine Editor — {project_name}"
        )));
    }

    // ── 2. Toolbar + 模式切换（工具栏会改 play_mode，所以先记下旧值）──
    let prev_mode = session.state.play_mode;
    panels::toolbar::show(&mut session.state, ctx);
    match (prev_mode, session.state.play_mode) {
        (PlayMode::Edit, PlayMode::Playing) | (PlayMode::Paused, PlayMode::Playing) => {
            if session.state.game_runtime.is_none() {
                session.state.enter_play_mode();
            }
        }
        (PlayMode::Playing, PlayMode::Edit) | (PlayMode::Paused, PlayMode::Edit) => {
            session.state.exit_play_mode();
        }
        _ => {}
    }

    // ── 3. Ctrl+S 保存场景 ──
    if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::S)) {
        let _ = session.state.save_scene();
    }

    // ── 3b. Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y 撤销重做 ──
    // 三种组合一次读完再分派：省得在同一个 input 闭包里重复查询键盘状态。
    let (want_undo, want_redo) = ctx.input(|i| {
        let ctrl = i.modifiers.ctrl;
        (
            ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
            ctrl && ((i.modifiers.shift && i.key_pressed(egui::Key::Z))
                || i.key_pressed(egui::Key::Y)),
        )
    });
    if want_undo {
        session.state.undo();
    }
    if want_redo {
        session.state.redo();
    }

    // ── 4. Physics step (only when actively playing) ──
    if session.state.play_mode == PlayMode::Playing {
        // 先把 egui 的键鼠状态灌进运行时 —— 必须发生在 physics_step **之前**，
        // 否则脚本这一帧读到的是上一帧的按键。
        if let Some(runtime) = session.state.game_runtime.as_mut() {
            input_bridge::sync(ctx, runtime.input_mut());
        }

        let dt = ctx.input(|i| i.predicted_dt);
        session.state.physics_step(dt);
        // Force continuous repaint during play — eframe only repaints
        // on input events by default, which would freeze physics.
        ctx.request_repaint();
    }

    // ── 5. Central dock area (tile tree with all panels) ──
    egui::CentralPanel::default().show(ctx, |ui| {
        let mut behavior = panels::dock::EditorBehavior {
            state: &mut session.state,
        };
        session.tree.ui(&mut behavior, ui);
    });

    // ── 6. Status bar (bottom) ──
    status_bar(ctx, &session.state);
}

/// 底部状态栏。
fn status_bar(ctx: &egui::Context, state: &EditorState) {
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Editor Framework v0.1.0");
            ui.separator();

            // Project
            ui.label(format!("Project: {}", state.project_name()));
            ui.separator();

            // Play mode indicator
            let btn_text = match state.play_mode {
                PlayMode::Edit => "Edit Mode",
                PlayMode::Playing => "▶ Playing",
                PlayMode::Paused => "⏸ Paused",
            };
            ui.label(btn_text);
            ui.separator();

            // Scene info
            ui.label(format!(
                "Scene: {} | {} entities",
                state.scene.name,
                state.scene.entities.len(),
            ));
            ui.separator();

            // Physics stats (only when playing)
            if state.play_mode != PlayMode::Edit {
                let (bodies, colliders) = state.physics_stats();
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

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

fn main() -> Result<(), eframe::Error> {
    env_logger::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_title("XGameEngine"),
        ..Default::default()
    };

    eframe::run_native(
        "XGameEngine",
        options,
        Box::new(|cc| Ok(Box::new(EditorApp::new(cc)))),
    )
}
