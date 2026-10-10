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
    /// 适配器。留着是为了查询**目标格式支持哪些采样数** —— MSAA 的采样数必须由
    /// 设备能力决定，硬开 4× 在不支持的格式上会让管线创建直接失败。
    adapter: Arc<wgpu::Adapter>,
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
    /// 上一次放行过的指针位置，供 [`drop_redundant_pointer_moves`] 去重用。
    last_pointer_pos: Option<egui::Pos2>,
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
            adapter: rs.adapter.clone(),
        });

        let mut app = Self {
            hub: ProjectHub::new(),
            session: None,
            gpu,
            last_pointer_pos: None,
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
            // MSAA 的采样数由设备能力决定：不支持的格式上硬开 4× 会让管线创建失败。
            let sample_count = xgame_render::scene_pipeline::supported_sample_count(
                &gpu.adapter,
                gpu.target_format,
            );
            log::info!("视口渲染：MSAA x{sample_count}");
            state.scene_renderer = Some(scene_renderer::SceneRenderer::new(
                &gpu.device,
                &gpu.queue,
                gpu.egui_renderer.clone(),
                gpu.target_format,
                project_root,
                sample_count,
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
        // ── 丢掉「位置没变」的指针移动事件 ──
        //
        // 这一条是**空闲时不再空转重绘**的关键。egui 在帧末只要看到「本帧有输入事件」
        // 就会请求**立即**重绘（`InputState::wants_repaint_after` 的 `!events.is_empty()`
        // 那一支）；而 Windows 在指针**停住**时依然会反复投递同一坐标的 `WM_MOUSEMOVE`，
        // egui-winit 逐条转成 `PointerMoved`。实测空闲时约 160 条/秒 → 编辑器一直以
        // 60fps 空转。
        //
        // 后果不只是费电：窗口的移动/缩放走的是 Windows 的模态循环，而应用每 16.7ms
        // 就要 present 一次并等 vsync，模态循环抢不到消息线程 —— 表现就是
        // **拖窗口卡顿**、拖拽/缩放窗口有延迟。
        //
        // 位置相同的 `PointerMoved` 不携带任何新信息（egui 的指针状态本来就已经停在
        // 那个位置上），丢掉它是语义上的 no-op，却能让 egui 不再无谓地排下一帧。
        drop_redundant_pointer_moves(ctx, &mut self.last_pointer_pos);

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
// 输入事件过滤
// ---------------------------------------------------------------------------

/// 丢掉位置与「上一次真正生效的位置」相同的 [`egui::Event::PointerMoved`]。
///
/// 为什么需要它见 [`EditorApp::update`] 顶部的说明：Windows 会在指针静止时反复
/// 投递同一坐标的 `WM_MOUSEMOVE`，而 egui 把"本帧有事件"当作"需要立刻再画一帧"
/// 的依据，于是空闲的编辑器也会 60fps 空转。
///
/// `PointerGone`（指针离开窗口）时要把记录清空 —— 否则指针离开再回到**同一个**
/// 位置时，那条事件会被误判成"没动"而被丢掉，egui 就永远不知道指针回来了。
fn drop_redundant_pointer_moves(ctx: &egui::Context, last_pos: &mut Option<egui::Pos2>) {
    ctx.input_mut(|input| {
        input.events.retain(|event| match event {
            egui::Event::PointerMoved(pos) => {
                if *last_pos == Some(*pos) {
                    false // 位置没变：丢弃
                } else {
                    *last_pos = Some(*pos);
                    true
                }
            }
            egui::Event::PointerGone => {
                *last_pos = None;
                true
            }
            _ => true,
        });
    });
}

// ---------------------------------------------------------------------------
// 编辑器界面
// ---------------------------------------------------------------------------

/// 绘制编辑器主界面（工具栏 → 物理步进 → 停靠面板 → 状态栏）。
fn editor_ui(session: &mut EditorSession, ctx: &egui::Context) {
    // ── 0. 选中索引收敛 ──
    // `selected_entity` 是按位置记的索引，而改实体数量/顺序的操作有十几处。任何一处
    // 漏同步，下一帧的 `entities[idx]` 就会 panic 掉整个编辑器。在每帧入口夹一次
    // 是 O(1) 的，把这类崩溃一次性堵死（详见 `EditorState::clamp_selection`）。
    session.state.clamp_selection();

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

        // 用**平滑过的真实帧间隔**推进物理，而不是 `predicted_dt`（那是"下一帧
        // 大概要多久"的预测）：固定步长累加器吃的就是这个 dt，预测偏了模拟的
        // 快慢就跟着偏。
        let dt = ctx.input(|i| i.stable_dt);
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
            // FPS 读真实帧间隔（`stable_dt`），别用 `predicted_dt` —— 后者是预测值，
            // 帧率真的掉下去时它可能仍然报 60，等于把卡顿藏起来。
            ui.label(format!(
                "FPS: {:.0}",
                ctx.input(|i| i.stable_dt).recip().round(),
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
        // 交换链只留 1 帧在飞。
        //
        // 默认值（`None` → wgpu 用 2）会让 present 最多排 3 帧：连续输入时表现为
        // "几帧挤在一起、然后一口气停 3 个 vsync"，鼠标悬停与拖窗口的手感就是
        // 一顿一顿的（实测帧间隔是 1.6ms / 54ms 的双峰，而不是均匀的 16.7ms）。
        // 收到 1 之后 present 最多等一个 vsync，输入到画面的延迟明显变小。
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            desired_maximum_frame_latency: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };

    eframe::run_native(
        "XGameEngine",
        options,
        Box::new(|cc| Ok(Box::new(EditorApp::new(cc)))),
    )
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 跑一帧，返回过滤之后还剩几个输入事件。
    ///
    /// 把 `max_passes` 钉成 1：egui 默认允许一帧跑两遍（多遍布局），第二遍会拿同一份
    /// 输入再走一次我们的过滤器 —— 那样测试量的就不是"过滤器本身"了。
    fn frame(
        ctx: &egui::Context,
        last: &mut Option<egui::Pos2>,
        events: Vec<egui::Event>,
    ) -> usize {
        ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::new(1).unwrap());
        let mut kept = 0;
        let _ = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                drop_redundant_pointer_moves(ctx, last);
                kept = ctx.input(|i| i.events.len());
            },
        );
        kept
    }

    /// 位置没变的 `PointerMoved` 必须被丢掉。
    ///
    /// 这是"空闲时不再 60fps 空转"的全部依据：Windows 在指针静止时会反复投递同一
    /// 坐标的 `WM_MOUSEMOVE`，egui 又把"本帧有事件"当成"要立刻再画一帧"的信号。
    /// 一旦这条过滤失效，编辑器在没人操作时也会一直重绘，窗口的移动/缩放模态循环
    /// 就会一直跟 present 抢消息线程 —— 也就是用户报的"拖窗口卡顿"。
    #[test]
    fn identical_pointer_moves_are_dropped_but_real_moves_pass() {
        let ctx = egui::Context::default();
        let mut last = None;
        let p = |x, y| egui::pos2(x, y);

        // 第一次出现 → 放行
        assert_eq!(
            frame(&ctx, &mut last, vec![egui::Event::PointerMoved(p(10.0, 20.0))]),
            1,
            "首次出现的指针位置应当放行"
        );
        // 同一位置再来一次（Windows 的空转来源）→ 丢掉
        assert_eq!(
            frame(&ctx, &mut last, vec![egui::Event::PointerMoved(p(10.0, 20.0))]),
            0,
            "位置没变的 PointerMoved 应当被丢掉"
        );
        // 同帧内重复投递同一位置 → 只留一条
        assert_eq!(
            frame(
                &ctx,
                &mut last,
                vec![
                    egui::Event::PointerMoved(p(30.0, 40.0)),
                    egui::Event::PointerMoved(p(30.0, 40.0)),
                    egui::Event::PointerMoved(p(30.0, 40.0)),
                ]
            ),
            1,
            "同一帧里的重复指针位置只该留一条"
        );
        // 真的移动了 → 放行
        assert_eq!(
            frame(&ctx, &mut last, vec![egui::Event::PointerMoved(p(31.0, 40.0))]),
            1,
            "指针真的移动时必须放行，否则界面会卡住不跟手"
        );
        // 其它事件一律不受影响
        assert_eq!(
            frame(&ctx, &mut last, vec![egui::Event::WindowFocused(true)]),
            1,
            "非指针事件不该被过滤"
        );
    }

    /// 指针离开窗口后再回到**同一个**位置，必须放行。
    ///
    /// 否则 `last_pos` 会一直留着旧值，那条"回来了"的事件被当成重复丢掉 ——
    /// egui 会以为指针还在窗口外，悬停高亮全部失效。
    #[test]
    fn pointer_returning_to_the_same_spot_passes_after_leaving() {
        let ctx = egui::Context::default();
        let mut last = None;
        let p = |x, y| egui::pos2(x, y);

        assert_eq!(frame(&ctx, &mut last, vec![egui::Event::PointerMoved(p(5.0, 5.0))]), 1);
        assert_eq!(frame(&ctx, &mut last, vec![egui::Event::PointerMoved(p(5.0, 5.0))]), 0);
        assert_eq!(frame(&ctx, &mut last, vec![egui::Event::PointerGone]), 1);
        assert_eq!(
            frame(&ctx, &mut last, vec![egui::Event::PointerMoved(p(5.0, 5.0))]),
            1,
            "离开再回到同一点必须放行，否则悬停会失效"
        );
    }
}
