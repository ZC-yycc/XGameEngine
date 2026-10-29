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
//!
//! # 无头工具（不开窗口，给 CI / AI 智能体用）
//! cargo run -p xgame-editor -- --check projects/SampleScene
//! cargo run -p xgame-editor -- --headless --frames 300 projects/SampleScene
//! cargo run -p xgame-editor -- --build projects/SampleScene
//!
//! # 界面截图（开窗口跑几帧，把上屏像素存成 PNG 后退出；调界面用）
//! cargo run -p xgame-editor -- projects/SampleScene --ui-shot shot.png
//! cargo run -p xgame-editor -- projects/SampleScene --ui-shot shot.png --ui-select Fox
//! cargo run -p xgame-editor -- projects/SampleScene --ui-shot play.png --ui-play
//! cargo run -p xgame-editor -- projects/SampleScene --ui-shot s.png --ui-size 1280x800
//! cargo run -p xgame-editor -- projects/SampleScene --ui-shot r.png --ui-autosave-recover
//! ```

mod autosave;
mod builder;
mod config;
mod dialogs;
mod frame_policy;
mod frame_probe;
mod game_runtime;
mod gizmo;
mod headless;
mod hot_reload;
mod hub;
mod input_bridge;
mod logging;
mod panels;
mod plugin_build;
mod plugin_session;
mod prefs;
mod profiler;
mod recents;
mod scene_renderer;
mod schema_cli;
mod script_api_cli;
mod state;

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use frame_policy::{FrameClock, FrameInputs, FramePlan, FramePolicy};
use hub::{HubAction, ProjectHub};
use panels::dock::PanelTab;
use state::{EditorState, ExitTarget, LogLevel, PlayMode};

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
    /// 「Preferences」窗口（打开位 + 草稿）。
    ///
    /// 窗口本身放在会话里（它要同时改 `EditorState` 与布局树），但**偏好数据**
    /// 在 `EditorApp`：那是跨项目、跨会话的，不随关闭项目消失。
    preferences_window: panels::preferences::PreferencesWindow,
    /// 命令面板（Ctrl+Shift+P / F1）：模糊搜索 + 执行编辑器动作。
    command_palette: panels::command_palette::CommandPalette,
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
    /// 帧调度策略（`XGAME_FRAME_POLICY` 可覆盖，缺省 Hybrid）。
    policy: FramePolicy,
    /// 帧调度状态：记住"最后一次输入"的时刻，用于连续出帧的宽限期。
    clock: FrameClock,
    /// 帧间隔探针（`XGAME_FRAME_LOG` 开关，默认关闭）。
    probe: frame_probe::FrameProbe,
    /// 界面截图请求（`--ui-shot`，开发/智能体工具）。
    ui_shot: Option<UiShot>,
    /// 编辑器级偏好（相机手感 / 网格 / 面板布局），跨项目共享。
    preferences: prefs::EditorPreferences,
    /// 本次运行是否允许写 `editor.ron`。
    ///
    /// `--ui-shot` 默认是**只读**的：它应该永远截出厂界面，不能被开发机上的
    /// 个人布局 / 网格开关影响（否则"老截图逐像素不变"这条验收就失效了）。
    /// 需要验证恢复逻辑时显式加 `--ui-restore-prefs`。
    save_preferences: bool,
    /// 本次运行是否允许写 `.autosave/` 副本。
    ///
    /// `--ui-shot` 默认是**只读**的（理由同 [`save_preferences`](Self::save_preferences)）：
    /// 截图会跑几十帧、还要等布局稳定，不该在用户的真实项目里留下自动保存
    /// 副本 —— 那会让下一次正常打开项目时多弹一个"发现自动保存"。
    write_autosave: bool,
}

/// 「跑几帧 → 抓上屏像素 → 落盘 → 退出」的界面截图请求。
///
/// 为什么不用 `PrintWindow` / 系统截图：编辑器窗口常常不在前台（`SetForegroundWindow`
/// 在无交互环境里会静默失败），而 UI 回归需要**同机逐字节可复现**的像素。
/// eframe/wgpu 的 `ViewportCommand::Screenshot` 拿到的就是这一帧真正提交上屏的内容，
/// 不受遮挡、缩放和 DWM 合成时机影响。
struct UiShot {
    path: std::path::PathBuf,
    /// 打开项目之后至少跑这么多帧再抓（等字体图集、布局和首帧视口渲染稳定）。
    after_frames: u32,
    /// 打开项目后按名字选中一个实体（截图里就能带上 Inspector 的真实内容）。
    select: Option<String>,
    frames_seen: u32,
    requested: bool,
    /// 上一帧的**逻辑**屏幕尺寸。连续两帧相同才认为布局稳定。
    ///
    /// 踩过的坑：窗口创建尺寸与 DPI 缩放的落定不在同一帧，抢在第一帧抓，
    /// 抓到的是一张"界面只画了左上角"的图（外框 1440×900，内容 936×579）。
    last_size: Option<egui::Vec2>,
    /// 连续稳定的帧数。
    stable: u32,
    /// 尺寸对不上时的重抓次数（见 [`EditorApp::service_ui_shot`]）。
    retries: u32,
    /// 抓图前把「项目设置」窗口打开。
    ///
    /// 这个窗口是模态的，只能靠点 ⚙ 打开 —— 没有这个开关就**永远截不到**
    /// 它，界面改版时它是唯一一块只能靠肉眼读代码猜排版的地方。
    open_settings: bool,
    /// 抓图前在资源浏览器里选中一个资产（详情区与导入设置才在画面里）。
    ///
    /// 没有选中时详情区整块不渲染 —— 这个开关是"资产详情 / 导入设置排版"
    /// 唯一可重复的验收手段（靠肉眼点选没法进自检回路）。
    asset: Option<String>,
    /// 抓图前先进 Play。
    ///
    /// Play 态有一整套只在运行时出现的界面：工具栏的暂停/停止组、视口 HUD、
    /// 状态栏的物理/音频段、Inspector 顶部的"热调参中"横幅。此前它们只能靠
    /// 人工按键后肉眼检查，改版时最容易漏。
    play: bool,
    /// 抓图前切到 Profiler 页签。
    ///
    /// 它与 Output Log 同一组，默认激活的是后者 —— 没有这个开关，Profiler
    /// 永远不出现在截图里（改版时等于没法自检）。
    profiler: bool,
    /// 抓图前打开 Preferences 窗口。
    open_preferences: bool,
    /// 抓图前打开命令面板（`--ui-command`）。
    ///
    /// 命令面板是键盘优先的浮层，肉眼点开后没法进"改 UI 前后逐像素对比"的
    /// 自检回路 —— 与 `--ui-settings` 同理，给开关才能截到它。
    /// 打开时**不**聚焦查询框：聚焦 TextEdit 的光标按墙钟闪烁，逐字节不可复现。
    open_command: bool,
    /// 抓图时**保留**「发现自动保存」恢复框。
    ///
    /// 默认**忽略**恢复框：它取决于磁盘上有没有 `.autosave/` 残留 —— 那是开发机
    /// 的外部状态，会让"同一份代码截出的图逐像素不变"这条验收失效。
    /// 要检查这扇窗的排版时显式加 `--ui-autosave-recover`（配合先手工放一份副本）。
    recover_autosave: bool,
    /// 抓图用的窗口逻辑尺寸（`--ui-size`，缺省 1440×900）。
    size: egui::Vec2,
}

impl EditorApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // ── 字体：赶在第一帧之前装好，否则项目枢纽第一屏就是一片方块字 ──
        // 只装这一次 —— `set_fonts` 会重建整张字形图集，不能每帧来。
        panels::theme::install_fonts(&cc.egui_ctx);
        // ── 主题：也只装这一次 ──
        // 关掉"跟随系统主题"：否则 egui 每帧都会按系统主题（这台机器是浅色）把
        // style 顶掉，过去"每帧重设主题"就是为了对抗那个覆盖。关掉之后每帧重建
        // 整个 `egui::Style` 纯属浪费。
        cc.egui_ctx.options_mut(|o| {
            // ThemePreference::Dark = 不跟随系统主题，固定用 dark_style。
            o.theme_preference = egui::ThemePreference::Dark;
        });
        panels::theme::install(&cc.egui_ctx);

        let gpu = cc.wgpu_render_state.as_ref().map(|rs| GpuHandles {
            device: rs.device.clone(),
            queue: rs.queue.clone(),
            egui_renderer: rs.renderer.clone(),
            target_format: rs.target_format,
            adapter: rs.adapter.clone(),
        });

        let now = Instant::now();
        let policy = FramePolicy::from_env();
        let probe = frame_probe::FrameProbe::from_env();
        log::info!(
            "帧调度策略：{policy:?}（探针{}）",
            if probe.enabled() { "开" } else { "关" }
        );

        // ── 命令行参数 ──
        // `xgame-editor <项目路径>` 直接进编辑器，跳过枢纽。
        // 路径无效时不退出——错误显示在枢纽界面上，用户还能另选一个。
        // `--ui-shot` 等开关与路径混排时按「第一个非开关参数是路径」解析。
        let launch = parse_launch_args(&std::env::args().skip(1).collect::<Vec<_>>());

        // `--ui-shot` 默认不读也不写 `editor.ron`：截图必须永远对应出厂界面，
        // 否则开发机换过一次布局，"改 UI 前后逐像素对比"就再也不成立了。
        // 验证恢复逻辑时加 `--ui-restore-prefs`。
        let restore_prefs = launch.ui_shot.is_none() || launch.restore_prefs;
        // 截图模式 = 只读：既不读个人偏好，也不写自动保存副本。
        let launched_for_shot = launch.ui_shot.is_some();
        let preferences = if restore_prefs {
            prefs::EditorPreferences::load()
        } else {
            prefs::EditorPreferences::default()
        };

        let mut app = Self {
            hub: ProjectHub::new(),
            session: None,
            gpu,
            last_pointer_pos: None,
            policy,
            clock: FrameClock::new(now),
            probe,
            ui_shot: launch.ui_shot,
            preferences,
            save_preferences: restore_prefs,
            // 截图路径不写自动保存：它只读，见字段文档。
            write_autosave: !launched_for_shot,
        };

        if let Some(arg) = launch.project {
            let action = app.hub.try_open(Path::new(&arg));
            if let HubAction::Open(project) = action {
                app.open_project(*project);
            }
        }

        // `--ui-settings`：把模态的项目设置窗口一并拉开（否则截不到）。
        if app
            .ui_shot
            .as_ref()
            .map(|s| s.open_settings)
            .unwrap_or(false)
        {
            if let Some(session) = app.session.as_mut() {
                session.state.settings_open = true;
            }
        }

        // `--ui-select <名字>`：截图前先选中实体（Inspector 才有内容可看）。
        if let Some(name) = app.ui_shot.as_ref().and_then(|s| s.select.clone()) {
            if let Some(session) = app.session.as_mut() {
                session.state.selected_entity = session
                    .state
                    .scene
                    .entities
                    .iter()
                    .position(|e| e.name == name);
            }
        }

        // `--ui-asset <路径>`：截图前在资源浏览器里选中一个资产。
        // 详情区（路径 / GUID / 导入设置）只在有选中时出现，不设它就永远截不到。
        // 材质额外把浮动编辑器打开 —— 截图要检查的正是四个字段与三个按钮。
        if let Some(asset) = app.ui_shot.as_ref().and_then(|s| s.asset.clone()) {
            if let Some(session) = app.session.as_mut() {
                if xgame_core::AssetKind::from_path(&asset) == Some(xgame_core::AssetKind::Material)
                {
                    session.state.material_editor_open = Some(asset.clone());
                }
                session.state.asset_selected = Some(asset);
            }
        }

        // `--ui-play`：进 Play 再截图（运行态专属的界面元素都在这里）。
        //
        // 放在选中之后：进入 Play 会克隆一份场景，实体顺序不变，选中索引仍然有效。
        //
        // **必须自己把 `play_mode` 推上去**：平时这条跃迁由 `editor_ui` 监听到
        // 工具栏改了枚举之后补上（见那里的 match），截图路径不经过工具栏，
        // 只调 `enter_play_mode()` 会得到一个"运行时在跑、界面写着 Edit Mode"
        // 的僵尸态 —— 状态栏 / Inspector 全按编辑态画，Play 态截图就白拍了。
        if app.ui_shot.as_ref().map(|s| s.play).unwrap_or(false) {
            if let Some(session) = app.session.as_mut() {
                session.state.enter_play_mode();
                // 插件工程会被推迟到编译完成（`game_runtime` 还是 None），
                // 那种情况下保持 Edit —— 与工具栏路径的语义一致。
                if session.state.game_runtime.is_some() {
                    session.state.play_mode = PlayMode::Playing;
                }
            }
        }

        // `--ui-profiler`：切到 Profiler 页签。
        //
        // 它在 Output Log 同一个页签组里，默认激活的是后者 —— 不切过去截到的
        // 还是日志，改版自检就等于什么都没看。放在 Play 之后：进 Play 会重建
        // 运行时，但布局树不动，谁先谁后都不影响选中页签，这里只是让"状态先
        // 摆好、再摆视图"的顺序保持统一。
        if app.ui_shot.as_ref().map(|s| s.profiler).unwrap_or(false) {
            if let Some(session) = app.session.as_mut() {
                panels::dock::activate(&mut session.tree, PanelTab::Profiler);
            }
        }

        // `--ui-preferences`：把偏好窗口拉开（否则这扇窗只能靠手点，改版时截不到）。
        if app
            .ui_shot
            .as_ref()
            .map(|s| s.open_preferences)
            .unwrap_or(false)
        {
            if let Some(session) = app.session.as_mut() {
                session.preferences_window.open(&app.preferences);
            }
        }

        // `--ui-command`：把命令面板拉开。不聚焦查询框 —— 聚焦 TextEdit 的
        // 光标按墙钟闪烁，会让"同一份代码截出的图逐字节不变"这条验收失效。
        if app.ui_shot.as_ref().map(|s| s.open_command).unwrap_or(false) {
            if let Some(session) = app.session.as_mut() {
                let state = &session.state;
                session.command_palette.open(state, false);
            }
        }

        app
    }

    /// 打开项目：构造编辑器状态 + 离屏渲染器，进入编辑器界面。
    fn open_project(&mut self, project: xgame_core::ProjectHandle) {
        let mut state = EditorState::new(project);

        // 截图模式**忽略**恢复框：它取决于磁盘上有没有 `.autosave/` 残留，
        // 那是开发机的外部状态。让它进画面，"同一份代码的截图逐像素不变"
        // 这条验收就不成立了。要专门检查这扇窗时加 `--ui-autosave-recover`。
        let shot_wants_recovery = self
            .ui_shot
            .as_ref()
            .map(|s| s.recover_autosave)
            .unwrap_or(false);
        if !self.write_autosave && !shot_wants_recovery {
            state.pending_autosave = None;
        }
        // 偏好是全局的：每个新会话都从当前值起步（相机速度 / 网格开关）。
        self.preferences.apply_to_state(&mut state);

        if let Some(gpu) = &self.gpu {
            let project_root = state.project.root().to_path_buf();
            // MSAA 的采样数由设备能力决定：不支持的格式上硬开 4× 会让管线创建失败。
            // **按 HDR 格式问** —— 场景 pass 画进 `Rgba16Float`（见 `xgame_render::postfx`），
            // 采样数是格式相关的量，拿交换链格式问出来的答案在 HDR 附件上不一定成立。
            let adapter_msaa = xgame_render::scene_pipeline::supported_sample_count(
                &gpu.adapter,
                xgame_render::postfx::HDR_FORMAT,
            );
            // 质量档位（`project.ron` 的 `quality` 节）：MSAA / 阴影默认分辨率 /
            // LOD 缩放。没配这一节 = 适配器最大采样（与引入档位之前一致）。
            let quality = state.project.manifest().quality.resolve(adapter_msaa);
            log::info!(
                "视口渲染：MSAA x{}（阴影 {}、LOD 缩放 {:.2}）",
                quality.msaa,
                quality.shadow_resolution,
                quality.lod_scale
            );
            state.scene_renderer = Some(scene_renderer::SceneRenderer::new(
                &gpu.device,
                &gpu.queue,
                gpu.egui_renderer.clone(),
                gpu.target_format,
                project_root,
                quality.msaa,
            ));
            if let Some(renderer) = state.scene_renderer.as_mut() {
                renderer.set_quality(quality);
            }
            // 渲染共享材质资产要项目句柄（`.mat` / 贴图 / 导入设置）。
            if let Some(renderer) = state.scene_renderer.as_mut() {
                renderer.set_project(Some(state.project.clone()));
            }
        }

        self.session = Some(EditorSession {
            // 存过就恢复，没存过 / 校验不过就回退出厂布局（见 prefs 模块文档）。
            tree: self.preferences.layout_for_startup(),
            state,
            preferences_window: panels::preferences::PreferencesWindow::new(),
            command_palette: panels::command_palette::CommandPalette::new(),
            // 空串保证进入编辑器后的第一帧一定会把标题刷成项目名。
            window_title: String::new(),
        });
    }

    /// 把当前会话的布局同步进偏好并写盘。
    ///
    /// 返回枢纽 / 关窗 / 退出时调用；没有会话（停在枢纽）时只写数值部分。
    /// `save_preferences == false`（`--ui-shot` 默认路径）时**什么都不做** ——
    /// 截图不能顺手改掉用户的 `editor.ron`。
    ///
    /// 布局同样要过 [`panels::dock::is_usable_layout`]：坏树不落盘，保留磁盘上
    /// 上一份合法布局（松手帧已经把会话内的坏树回滚过了，这里是最后一道闸）。
    fn persist_preferences(&mut self) {
        if !self.save_preferences {
            return;
        }
        let changed = match self.session.as_ref() {
            Some(session) if panels::dock::is_usable_layout(&session.tree) => {
                self.preferences.sync_layout(&session.tree)
            }
            Some(_) => {
                log::warn!("退出时的面板布局不合法，保留磁盘上原有布局");
                false
            }
            None => false,
        };
        // 布局没变就不写盘：同一个会话反复关闭 / 退出不该重复写。
        // 数值偏好走 Apply 时已经写过；ResetLayout 也当场写过。
        if changed {
            self.preferences.save();
        }
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 本帧闭包的起点：探针用它算 `w_us`（工作）与 dt 的差值（上屏/唤醒段）。
        let t_in = Instant::now();

        // ── 丢掉「位置没变」的指针移动事件 ──
        //
        // egui 在帧末只要看到「本帧有输入事件」就会请求**立即**重绘；而 Windows 在指针
        // **停住**时依然会反复投递同一坐标的 `WM_MOUSEMOVE`，egui-winit 逐条转成
        // `PointerMoved`（实测空闲时约 160 条/秒）。位置相同的 `PointerMoved` 不携带任何
        // 新信息（egui 的指针状态本来就已经停在那儿），丢掉它是语义上的 no-op。
        //
        // 这一条**只是**为了别让这些幽灵事件干扰"本帧有没有真实输入"的判断 ——
        // 「要不要连续出帧」现在由 `frame_policy` 明确决策，不再靠这个副作用。
        drop_redundant_pointer_moves(ctx, &mut self.last_pointer_pos);

        // ── 引擎日志进控制台 ──
        //
        // 上一帧（以及本帧开始之前）产生的 `log::warn!` / `log::info!` 在这里落地：
        // 资产装不上、骨骼超上限、脚本报错……没有这一步它们只进 stderr，而双击
        // exe 启动的用户**没有终端**，等于什么都看不到。策略见 `logging` 模块头。
        if let Some(session) = self.session.as_mut() {
            for record in logging::take_records() {
                session
                    .state
                    .log(LogLevel::from_log(record.level), record.console_text());
            }
        }

        // ── 帧调度（这一帧之后还要不要再排一帧）──
        //
        // 编辑器里嵌着实时 3D 视口，不能像表单那样"没输入就不画"：那样鼠标一停画面就冻住，
        // 交互也会因为"每个事件才算一帧"而发涩。但也不能无脑永远满速 —— 那会在真空闲时
        // 白烧 CPU，还会和窗口移动/缩放的模态循环抢消息线程。
        // 具体取舍见 `frame_policy` 模块头。
        let now = Instant::now();
        let had_input = ctx.input(|i| !i.events.is_empty());
        if had_input {
            self.clock.note_input(now);
        }
        let inputs = FrameInputs {
            // 工具栏要等 `editor_ui` 跑完才知道用户点没点 Play，这里读的是本帧开始时的
            // 状态 —— 差一帧无所谓：点 Play 本身就是输入，那一帧一定已经在连续出帧了。
            playing: self
                .session
                .as_ref()
                .is_some_and(|s| s.state.play_mode == PlayMode::Playing),
            interacting: ctx.input(|i| i.pointer.any_down()),
            had_input,
            // 视口动画：`viewport_animating` 会顺手把动画实体的模型预加载到位
            // （"剪辑存在吗"要先有资产才能答），拿不到就是没在播 —— 坏资产
            // 不会把编辑器拖进满速空转。
            animating: self.session.as_mut().is_some_and(|s| {
                let state = &mut s.state;
                let anim = match (&mut state.scene_renderer, &state.scene) {
                    (Some(renderer), scene) => renderer.viewport_animating(scene),
                    (None, _) => false,
                };
                // 粒子也算"在动"：视角不动、只有粒子在飞时，缺这一条就会被
                // 当成空闲停下来 —— 火焰会一格一格地跳。
                anim || state.particles_animating()
            }),
        };
        let live = self.clock.plan(self.policy, inputs, now) == FramePlan::Now;
        if live {
            ctx.request_repaint();
        }

        // ── 关窗拦截：有未保存改动先确认 ──
        //
        // egui 的语义：`close_requested()` 为真的**这一帧**不发 CancelClose，
        // 帧末应用就直接退出。所以必须在 editor_ui 之前拦下 —— 用户点 X 的
        //那一帧取消关闭、改弹确认框；用户放行后再由 `editor_ui` 里的弹窗
        // 发 `ViewportCommand::Close` 真正关窗。枢纽界面上没有可丢的东西，不拦。
        let pass_nr = ctx.cumulative_pass_nr();
        // 借用 `session` 期间不能再碰 `self`，先把本帧要用的开关取出来。
        let write_autosave = self.write_autosave;
        if ctx.input(|i| i.viewport().close_requested()) {
            if let Some(session) = self.session.as_mut() {
                if session.state.exit_dialog.is_none() && session.state.scene_dirty_memo(pass_nr) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    session.state.exit_dialog = Some(ExitTarget::Quit);
                } else if session.state.exit_dialog.is_none() && write_autosave {
                    // 无脏、直接放行 = 正常退出。同样要清副本（"改了又撤销回原样"
                    // 的场景下，盘上可能还留着更早的一份）。
                    session.state.drop_autosave_of_current();
                }
            }
        }

        // ── 编辑器界面 ──
        let mut close_requested = false;
        if let Some(session) = self.session.as_mut() {
            editor_ui(
                session,
                &mut self.preferences,
                self.save_preferences,
                self.write_autosave,
                ctx,
                live,
            );

            // 确认框的放行凭证（保存成功 / 用户明确选了"不保存"）。
            // 存在的意义见 `EditorState::exit_confirmed`："不保存"之后场景
            // 依然是脏的，只看脏标记会把确认框复弹到死。
            if let Some(target) = session.state.exit_confirmed.take() {
                // 会话到此结束：`.autosave/` 副本只服务**崩溃**。用户主动退出
                // （保存过，或明确选了"不保存"）说明他已经做过决定，下次启动
                // 不该再被问一遍"要不要恢复上次的工作"。崩溃时走不到这里，
                // 副本留在盘上 —— 那才是恢复流程的入口。
                if write_autosave {
                    session.state.drop_autosave_of_current();
                }
                match target {
                    ExitTarget::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    ExitTarget::Hub => close_requested = true,
                }
            } else if session.state.return_to_hub {
                session.state.return_to_hub = false;
                if session.state.scene_dirty_memo(pass_nr) {
                    session.state.exit_dialog = Some(ExitTarget::Hub);
                    // 本帧的编辑器 UI 已经画完，不补一帧的话确认框要等下次
                    // 输入才出现（帧调度 idle 时不会自己出帧，等于"点了没反应"）。
                    ctx.request_repaint();
                } else {
                    close_requested = true;
                }
            }
        }

        if close_requested {
            // 关闭会话前把当前布局带进偏好并落盘：这是"下次打开还是这套布局"
            // 的唯一保证。`--ui-shot` 默认 `save_preferences == false`，不会写。
            self.persist_preferences();
            self.session = None;
        }

        // ── 项目枢纽 ──
        if self.session.is_none() {
            let action = self.hub.ui(ctx);
            if let HubAction::Open(project) = action {
                self.open_project(*project);
            }
        }

        let pointer = ctx.input(|i| i.pointer.latest_pos());
        // 分组用：rt=0 表示这是同一次 ctx.run 的丢弃重跑（第二遍），不是新帧。
        let raw_time = ctx.input(|i| i.raw.time.is_some());
        let will_discard = ctx.will_discard();
        self.probe.tick(
            t_in,
            Instant::now(),
            live,
            pointer.map(|p| (p.x, p.y)),
            raw_time,
            will_discard,
            pass_nr,
        );

        // 界面截图（`--ui-shot`）：放在最后一帧的所有 UI 之后 —— 抓到的就是
        // 用户真正看到的那一帧（含状态栏与弹窗层）。
        self.service_ui_shot(ctx);
    }

    /// eframe 在窗口真正销毁前调用一次。
    ///
    /// 关窗路径有两条（直接点 X / 未保存确认后放行），其中"干净时点 X"不经过
    /// `close_requested` 分支；这里兜住最后一次布局保存。
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.persist_preferences();
    }
}

// ---------------------------------------------------------------------------
// 启动参数（窗口模式）
// ---------------------------------------------------------------------------

/// 窗口模式的启动参数。
#[derive(Default)]
struct LaunchArgs {
    /// 直接打开的项目路径（第一个非开关参数）。
    project: Option<String>,
    /// 界面截图请求。
    ui_shot: Option<UiShot>,
    /// `--ui-shot` 时也读（写）真实的 `editor.ron`。
    ///
    /// 默认截图**不碰**用户偏好：同一份代码在任何开发机上都要截出同一张图。
    /// 要验证"恢复布局 / 网格关闭"这类偏好效果时才显式打开。
    restore_prefs: bool,
}

/// 解析窗口模式的启动参数。
///
/// 只认 `--ui-shot <png>` / `--ui-frames <n>` / `--ui-select <实体名>` /
/// `--ui-asset <资产路径>` / `--ui-settings` / `--ui-play` / `--ui-profiler` /
/// `--ui-preferences` / `--ui-command` / `--ui-restore-prefs` / `--ui-size <WxH>`
/// 十一个开关，其余非开关参数按顺序取**第一个**当项目路径。
/// `--check` / `--headless` 在 `main` 更早的分支就分流了，不会走到这里。
fn parse_launch_args(raw: &[String]) -> LaunchArgs {
    let mut out = LaunchArgs::default();
    let mut shot_path: Option<std::path::PathBuf> = None;
    let mut after_frames = 12u32;
    let mut select: Option<String> = None;
    let mut asset: Option<String> = None;
    let mut open_settings = false;
    let mut play = false;
    let mut profiler = false;
    let mut open_preferences = false;
    let mut open_command = false;
    let mut recover_autosave = false;
    let mut size = egui::vec2(SHOT_WIDTH, SHOT_HEIGHT);

    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--ui-shot" => {
                if let Some(path) = raw.get(i + 1) {
                    shot_path = Some(std::path::PathBuf::from(path));
                    i += 1;
                } else {
                    log::error!("--ui-shot 需要一个 PNG 路径");
                }
            }
            "--ui-frames" => {
                if let Some(n) = raw.get(i + 1).and_then(|s| s.parse().ok()) {
                    after_frames = n;
                    i += 1;
                } else {
                    log::error!("--ui-frames 需要一个正整数");
                }
            }
            "--ui-select" => {
                if let Some(name) = raw.get(i + 1) {
                    select = Some(name.clone());
                    i += 1;
                } else {
                    log::error!("--ui-select 需要一个实体名");
                }
            }
            "--ui-asset" => {
                if let Some(path) = raw.get(i + 1) {
                    asset = Some(path.clone());
                    i += 1;
                } else {
                    log::error!("--ui-asset 需要一个资产路径（项目相对）");
                }
            }
            "--ui-settings" => {
                open_settings = true;
            }
            "--ui-play" => {
                play = true;
            }
            "--ui-profiler" => {
                profiler = true;
            }
            "--ui-preferences" => {
                open_preferences = true;
            }
            "--ui-command" => {
                open_command = true;
            }
            "--ui-restore-prefs" => {
                out.restore_prefs = true;
            }
            "--ui-autosave-recover" => {
                recover_autosave = true;
            }
            "--ui-size" => {
                // `1440x900` / `1280X800` 都收；解析失败就保持缺省。
                match raw.get(i + 1).map(|s| parse_size(s)) {
                    Some(Some(parsed)) => {
                        size = parsed;
                        i += 1;
                    }
                    Some(None) => log::error!("--ui-size 需要 <宽>x<高>，例如 1280x800"),
                    None => log::error!("--ui-size 需要 <宽>x<高>，例如 1280x800"),
                }
            }
            other if !other.starts_with("--") => {
                if out.project.is_none() {
                    out.project = Some(other.to_owned());
                }
            }
            other => log::warn!("忽略未知参数: {other}"),
        }
        i += 1;
    }

    out.ui_shot = shot_path.map(|path| UiShot {
        path,
        after_frames,
        select,
        asset,
        frames_seen: 0,
        requested: false,
        last_size: None,
        stable: 0,
        retries: 0,
        open_settings,
        play,
        profiler,
        open_preferences,
        open_command,
        recover_autosave,
        size,
    });
    out
}

/// 解析 `1280x800` 形式的尺寸（`x` / `X` / `*` 都收，前后空格忽略）。
///
/// 单独抽出来是为了能测：`--ui-size` 是排版回归的入口，参数写错了会
/// **静默**退化成缺省尺寸 —— 那几轮回归就白跑一遍还不知道。
fn parse_size(text: &str) -> Option<egui::Vec2> {
    let lower = text.trim().to_ascii_lowercase();
    let (w, h) = lower.split_once(['x', '*'])?;
    let w: f32 = w.trim().parse().ok()?;
    let h: f32 = h.trim().parse().ok()?;
    // 比 480×360 还小的窗口画不下四列面板（Dock 会挤成负宽度），当参数错误处理。
    (w >= 480.0 && h >= 360.0 && w.is_finite() && h.is_finite()).then(|| egui::vec2(w, h))
}

impl EditorApp {
    /// 推进 `--ui-shot` 状态机：等布局稳定 → 请求截图 → 收到像素落盘 → 关窗。
    ///
    /// 等待期间必须自己 `request_repaint`：帧调度在静态界面上会停帧，
    /// 停帧之后截图命令可能永远等不到下一帧去处理。
    fn service_ui_shot(&mut self, ctx: &egui::Context) {
        let Some(shot) = self.ui_shot.as_mut() else {
            return;
        };

        // 第一帧就把窗口要成固定尺寸：截图要逐字节可比，不能受"上次窗口大小 /
        // 落在哪台显示器"影响（不同显示器的 DPI 缩放会让同一份界面差一个像素）。
        if shot.frames_seen == 0 {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(shot.size));
        }
        shot.frames_seen += 1;

        // 尺寸稳定判定：连着一帧没变才算布局落定。
        let size = ctx.screen_rect().size();
        if shot.last_size == Some(size) {
            shot.stable += 1;
        } else {
            shot.last_size = Some(size);
            shot.stable = 0;
        }

        if !shot.requested {
            if shot.frames_seen >= shot.after_frames && shot.stable >= 2 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                shot.requested = true;
            }
            ctx.request_repaint();
            return;
        }

        let image = ctx.input(|i| {
            i.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });

        let Some(image) = image else {
            // 截图命令已经发出，但像素还没回来：别停帧。
            ctx.request_repaint();
            return;
        };

        // 上屏纹理的尺寸必须等于「逻辑尺寸 × 缩放系数」。对不上说明这一帧
        // 涂的是一张还没跟上的旧目标（缩放刚变 / 窗口刚 resize），丢掉重抓。
        let ppp = ctx.pixels_per_point();
        let expected = [
            (size.x * ppp).round().max(1.0) as usize,
            (size.y * ppp).round().max(1.0) as usize,
        ];
        if image.size != expected && shot.retries < 3 {
            shot.retries += 1;
            log::warn!(
                "界面截图尺寸不符（拿到 {:?}，期望 {expected:?}，ppp={ppp:.3}），重抓一次",
                image.size
            );
            shot.requested = false;
            shot.stable = 0;
            ctx.request_repaint();
            return;
        }

        let (width, height) = (image.size[0], image.size[1]);
        let mut rgba = Vec::with_capacity(width * height * 4);
        for pixel in &image.pixels {
            rgba.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), pixel.a()]);
        }

        match xgame_render::screenshot::save_rgba_png(
            &shot.path,
            &rgba,
            width as u32,
            height as u32,
        ) {
            Ok(()) => log::info!("界面截图已保存: {} ({width}x{height})", shot.path.display()),
            Err(error) => log::error!("界面截图落盘失败: {error:#}"),
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// `--ui-shot` 抓图时的窗口逻辑尺寸（16:10，够放下四列面板 + 视口）。
const SHOT_WIDTH: f32 = 1440.0;
const SHOT_HEIGHT: f32 = 900.0;

// ---------------------------------------------------------------------------
// 输入事件过滤
// ---------------------------------------------------------------------------

/// 丢掉两类"不该算作用户输入"的指针事件。
///
/// 1. **位置没变的 [`egui::Event::PointerMoved`]**：Windows 在指针静止时反复投递
///    同一坐标的 `WM_MOUSEMOVE`，而 egui 把"本帧有事件"当作"需要立刻再画一帧"
///    的依据，于是空闲的编辑器也会 60fps 空转。
///
///    `PointerGone`（指针离开窗口）时要把记录清空 —— 否则指针离开再回到**同一个**
///    位置时，那条事件会被误判成"没动"而被丢掉，egui 就永远不知道指针回来了。
///
/// 2. **[`egui::Event::MouseMoved`]（相对位移）**：来自 winit 的
///    `DeviceEvent::MouseMotion` 原始输入，和 `PointerMoved`（绝对位置）是两回事。
///    光电鼠标的传感器噪声会以 ±1~2 像素的 `MouseMoved` 持续灌进来，把帧调度的
///    "刚有输入"宽限期无限续命 —— 实测指针完全静止时编辑器仍以 ~16fps 出帧，
///    用事件日志定位源头就是它。编辑器没有任何地方消费原始 delta（相机旋转用的是
///    `pointer.delta()`，由 `PointerMoved` 产生），egui 内部也只把它累计进
///    `input.raw.motion`，丢弃是安全的。
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
            // 原始位移（传感器噪声的主要载体）：丢弃。
            egui::Event::MouseMoved(_) => false,
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
///
/// `live` 是**本帧的帧调度决策**（要不要连续出帧）。状态栏用它把 FPS 显示成
/// 「此刻在出帧」还是「已空闲」—— 免得空闲时把上一帧的最后一次读数一直挂在那儿，
/// 看起来像卡死了。
fn editor_ui(
    session: &mut EditorSession,
    editor_prefs: &mut prefs::EditorPreferences,
    save_preferences: bool,
    write_autosave: bool,
    ctx: &egui::Context,
    live: bool,
) {
    // Profiler 的"主循环工作"起点：从这一帧进编辑器界面算到退出。不含
    // `update()` 前半段（指针去重 + 帧调度决策），那两段是常数级、且与界面无关。
    let t_frame_start = Instant::now();

    // ── 0. 选中索引收敛 ──
    // `selected_entity` 是按位置记的索引，而改实体数量/顺序的操作有十几处。任何一处
    // 漏同步，下一帧的 `entities[idx]` 就会 panic 掉整个编辑器。在每帧入口夹一次
    // 是 O(1) 的，把这类崩溃一次性堵死（详见 `EditorState::clamp_selection`）。
    session.state.clamp_selection();

    // ── 0b. Rust 插件后台编译轮询（P0.5）──
    // 编译在独立线程里跑，这里只收结果。放在这一步（工具栏之前）是刻意的：
    // 编译完成时可能要**立刻进 Play**，而进 Play 的状态跃迁检测在工具栏之后 ——
    // 先把状态摆好，同一帧内就能完成"编译完成 → 进 Play"。
    session.state.poll_plugin_build();
    if session.state.plugin_building() {
        // 空闲时编辑器会停止出帧（省电），编译中必须持续唤醒 —— 否则结果
        // 永远没人来收，编译其实早完成了、界面还停在"编译中"。
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    // ── 1. 窗口标题：项目名 + 未保存星号（仅变化时发，见 EditorSession::window_title）──
    // "*" 是**标题栏级**的未保存提示：标题是唯一永远在视野里的位置，工具栏
    // 按钮会被弹窗挡住。与工具栏共用同一份按帧 memo 的脏检查（不重复比较）。
    let title_name = {
        let project_name = session.state.project_name().to_owned();
        if session.state.scene_dirty_memo(ctx.cumulative_pass_nr()) {
            format!("{project_name} *")
        } else {
            project_name
        }
    };
    if session.window_title != title_name {
        session.window_title = title_name.clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "XGameEngine Editor — {title_name}"
        )));
    }

    // ── 2. Toolbar + 模式切换（工具栏会改 play_mode，所以先记下旧值）──
    let prev_mode = session.state.play_mode;
    panels::toolbar::show(&mut session.state, ctx);
    match (prev_mode, session.state.play_mode) {
        (PlayMode::Edit, PlayMode::Playing) | (PlayMode::Paused, PlayMode::Playing)
            if session.state.game_runtime.is_none() =>
        {
            session.state.enter_play_mode();
        }
        (PlayMode::Playing, PlayMode::Edit) | (PlayMode::Paused, PlayMode::Edit) => {
            session.state.exit_play_mode();
        }
        _ => {}
    }

    // 工具栏只置请求位，窗口要同时看到布局树，所以在这里消费。
    if session.state.open_preferences {
        session.state.open_preferences = false;
        session.preferences_window.open(editor_prefs);
    }

    // ── 3. Ctrl+S 保存 / Ctrl+Shift+S 另存为 ──
    // 一次读完再分派：省得在同一个 input 闭包里重复查询键盘状态。
    let (want_save, want_save_as) = ctx.input(|i| {
        let ctrl_s = i.modifiers.ctrl && i.key_pressed(egui::Key::S);
        (ctrl_s && !i.modifiers.shift, ctrl_s && i.modifiers.shift)
    });
    if want_save {
        let _ = session.state.save_scene();
    }
    if want_save_as {
        session.state.save_as_open = true;
        session.state.save_as_name = session.state.scene.name.clone();
    }

    // ── 3a. 自动保存（只写 `.autosave/` 副本，不动脏标记）──
    //
    // 放在 Ctrl+S 之后：手动保存会顺手删掉副本。顺序反了就是"先写一份、紧接着
    // 删掉"，白跑一次同步 IO。
    if write_autosave {
        let now = Instant::now();
        session.state.autosave_tick(now);
        // ── 3a-2. 资产 / 脚本热重载 ──
        //
        // 与自动保存同一条纪律：只在**交互会话**里跑。`--ui-shot` 默认只读，
        // 扫描到开发机上的临时文件会让截图内容不可复现。
        session.state.hot_reload_tick(now);
        // 空闲停帧时不会有人再来问"到点没"。这里排一次**定时**唤醒
        // （`request_repaint_after` 不强制立刻重绘），自动保存才能准点发生，
        // 外部改脚本 / 资产也才能在 1 秒内被发现。两条各排一次，
        // egui 取其中最早的时刻。只在 `!live` 排：连续出帧时本来就不缺 frame。
        if !live {
            if let Some(wait) = session.state.autosave_wait(now) {
                ctx.request_repaint_after(wait);
            }
            ctx.request_repaint_after(session.state.hot_reload_wait(now));
        }
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

    // ── 3c. Ctrl+Shift+P / F1 命令面板 ──
    //
    // 刻意**不过 `wants_keyboard_input`**：命令面板的意义就是在任何地方
    // （包括正在输入框里打字时）都能一把召出。与 Ctrl+S 同一档全局快捷键。
    // 再按一次收起（toggle）—— 面板查询框聚焦时按 Ctrl+Shift+P 不该被
    // TextEdit 吃掉，那是"关掉面板"的预期路径。
    let want_palette = ctx.input(|i| {
        (i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::P))
            || i.key_pressed(egui::Key::F1)
    });
    if want_palette {
        if session.command_palette.is_open() {
            session.command_palette.close();
        } else {
            let focus_query = true;
            session.command_palette.open(&session.state, focus_query);
        }
    }

    // ── 4. Physics step (only when actively playing) ──
    //
    // 这一段单独计时进 Profiler 面板（"运行时推进"）：脚本、物理、音频都在
    // `World::update` 里，Play 态下它是唯一会随场景规模增长的耗时块。
    let mut runtime_us = std::time::Duration::ZERO;
    if session.state.play_mode == PlayMode::Playing {
        // 先把 egui 的键鼠状态灌进运行时 —— 必须发生在 physics_step **之前**，
        // 否则脚本这一帧读到的是上一帧的按键。
        // UI 指针：egui 的逻辑点 → 视口物理像素。
        //
        // 用**上一帧**的视口矩形：这一步在面板绘制之前，本帧的布局还没跑。
        // 面板停靠变化的那一帧可能差几像素，之后的帧都是准的 —— 与 egui
        // 自己处理输入的方式一致。
        let ui_pointer = input_bridge::viewport_pointer(
            session.state.viewport_rect,
            ctx.pixels_per_point(),
            ctx.input(|i| i.pointer.latest_pos()),
        );
        let ui_viewport = session.state.viewport_pixel_size;

        if let Some(runtime) = session.state.game_runtime.as_mut() {
            input_bridge::sync(ctx, runtime.input_mut());
            runtime.set_ui_viewport(ui_viewport);
            runtime.set_ui_pointer(ui_pointer);
        }

        // 用**平滑过的真实帧间隔**推进物理，而不是 `predicted_dt`（那是"下一帧
        // 大概要多久"的预测）：固定步长累加器吃的就是这个 dt，预测偏了模拟的
        // 快慢就跟着偏。
        let dt = ctx.input(|i| i.stable_dt);
        let t_runtime = Instant::now();
        session.state.physics_step(dt);
        runtime_us = t_runtime.elapsed();
        // 这里**不再**手动 `request_repaint()`：Play 模式下 `frame_policy` 会持续
        // 排帧（见 `FrameInputs::playing`）。把"什么时候出帧"集中到一处决策，
        // 免得散落在各处的 `request_repaint` 互相打架。
    }

    // ── 5. Status bar (bottom) ──
    //
    // **必须在停靠区之前画**：`CentralPanel` 会把"当时剩下的整块矩形"全吃掉，
    // 之后再加的 `TopBottomPanel` 只会叠在它上面、不会让它缩短。以前顺序反了，
    // 结果停靠区底部有 24px 永远压在状态栏底下 —— 面板里靠底排的内容（Profiler
    // 的绘制读数）就整行画进不可见区。egui 的文档也写着：`CentralPanel` 必须最后加。
    status_bar(ctx, &session.state, live);

    // ── 6. Central dock area (tile tree with all panels) ──
    //
    // 停靠区这一段也进 Profiler：各面板的排版 + **视口渲染提交**都在这里，
    // 编辑态的绝大部分耗时就在这一点上（Play 态才是运行时推进）。
    let t_ui = Instant::now();
    egui::CentralPanel::default().show(ctx, |ui| {
        let mut behavior = panels::dock::EditorBehavior {
            state: &mut session.state,
        };
        session.tree.ui(&mut behavior, ui);
    });
    let ui_us = t_ui.elapsed();

    // 拖完页签 / 分隔条就保存布局。只在**松手那一帧**比较整棵树：
    // 拖拽中的每一帧树都不同，逐帧保存会变成高频磁盘写；而每帧比较需要
    // 深比较 + 克隆，也没必要。没拖过时 `layout` 与树不同（首次运行为 None），
    // 第一次松手会顺手把默认布局写下来 —— 这正是"从没保存过"该有的结果。
    //
    // **先校验再落盘**：拖页签条背景会拖动整个 Tabs 容器，egui_tiles 允许把
    // 容器当页签插进任意 Tabs（`insert_at` 的 `Tabs` 分支）——松手帧 `move_tile`
    // 刚把这种坏结构造出来，原样写进 `editor.ron` 会让下次启动首帧闪退。坏树
    // 不落盘，并回滚到上次合法布局（`layout_for_startup` 对坏文件回退出厂）。
    if save_preferences && ctx.input(|i| i.pointer.any_released()) {
        if panels::dock::is_usable_layout(&session.tree) {
            if editor_prefs.sync_layout(&session.tree) {
                editor_prefs.save();
            }
        } else {
            log::warn!("拖拽产生了非法的面板布局（页签中混入容器 / 非法分隔比例），已回滚");
            session.tree = editor_prefs.layout_for_startup();
        }
    }

    // ── 6.5. 资源拖拽收尾 ──
    //
    // 放置动作在视口里做（它知道射线落点），这里只负责"松手即清空"。
    // 放在面板绘制之后：视口同帧已经拿到 payload；而面板被拖成非活动页签、
    // 整帧没绘制时，这一行仍然会执行，不会留下跟着鼠标的幽灵。
    if session.state.asset_drag.is_some() && ctx.input(|i| i.pointer.any_released()) {
        session.state.asset_drag = None;
    }

    // ── 7. 应用级弹窗 ──
    // 画在最后：确认框 / 另存为要盖在设置窗口之上（同层后画的在上）。
    panels::assets::show_overlays(&mut session.state, ctx);

    if let Some(action) =
        session
            .preferences_window
            .show(&mut session.state, &mut session.tree, ctx)
    {
        match action {
            panels::preferences::PreferencesAction::Apply(next) => {
                if save_preferences {
                    next.save();
                }
                *editor_prefs = *next;
            }
            panels::preferences::PreferencesAction::ResetLayout => {
                session.tree = panels::dock::create_tile_tree();
                editor_prefs.layout = Some(session.tree.clone());
                if save_preferences {
                    editor_prefs.save();
                }
                session.state.log_info("Restored default panel layout");
            }
        }
    }

    panels::settings::show(&mut session.state, ctx);
    dialogs::show_save_as_dialog(&mut session.state, ctx);
    dialogs::show_new_scene_dialog(&mut session.state, ctx);
    dialogs::show_scene_change_dialog(&mut session.state, ctx, ctx.cumulative_pass_nr());
    if let Some(target) =
        dialogs::show_exit_dialog(&mut session.state, ctx, ctx.cumulative_pass_nr())
    {
        session.state.exit_confirmed = Some(target);
    }

    // 崩溃恢复框画在最末：它只在打开项目后的头几帧存在，盖在其他弹窗之上
    // 才不会被"未保存更改"压住（两个都开着时，恢复框是更紧急的那个）。
    dialogs::show_autosave_recovery_dialog(&mut session.state, ctx);

    // 命令面板画在**最顶层**（在恢复框之后）：它是用户主动召出的瞬时浮层，
    // 打开期间必须盖住一切 —— 被别的弹窗压住一半的搜索框没法用。
    session
        .command_palette
        .show(&mut session.state, &mut session.tree, ctx);

    // ── 8. 帧采样（Profiler 面板的数据面）──
    //
    // 放在最末：本帧每一段耗时都已测完，样本是完整的一份。采样本身只写一个
    // 预分配槽位（零分配），面板没打开时照样采 —— 换来"切过去就能看到刚才那段"，
    // 而不是从打开那一刻才开始有数据。`live` 原样记下：空闲停帧那一帧的 `dt`
    // 是唤醒间隔，统计时必须与连续帧分开（见 `profiler` 模块文档）。
    let work_us = t_frame_start.elapsed();
    let dt_us = (ctx.input(|i| i.stable_dt).max(0.0) * 1_000_000.0) as u128;
    session
        .state
        .profiler
        .record(profiler::FrameSample::from_micros(
            dt_us,
            work_us.as_micros(),
            runtime_us.as_micros(),
            ui_us.as_micros(),
            live,
        ));
}

/// 底部状态栏。
///
/// UE 的状态栏是一条**比面板深**的窄条（24px），文字一律 11px、标签暗、数值亮，
/// 段与段之间用 1px 竖线分开；右侧固定放"不常看但必须能看到"的读数（FPS / 版本）。
/// 这里刻意不用 `ui.separator()`：它是横线，塞进水平布局里会把行高撑到 30px 以上。
/// 状态栏的自动保存读数；`None` = 不显示。
///
/// 三条都不显示：非编辑态（Play 时不写副本）、自动保存被关掉、还没成功写过
/// 任何一次 —— 空状态下多一个"自动保存 —"只是噪音。
fn autosave_label(state: &EditorState) -> Option<(String, egui::Color32)> {
    use crate::panels::theme::colors;
    let autosave = state.autosave_state();
    if !state.is_editing() || !autosave.is_enabled() {
        return None;
    }
    if autosave.last_error().is_some() {
        return Some(("自动保存失败".to_owned(), colors::WARN));
    }
    let elapsed = autosave.last_success()?.elapsed();
    Some((
        format!("自动保存 {}", crate::autosave::relative_label(elapsed)),
        colors::TEXT_DIM,
    ))
}

fn status_bar(ctx: &egui::Context, state: &EditorState, live: bool) {
    use crate::panels::{icons, theme};

    let frame = egui::Frame::default()
        .fill(theme::colors::PANEL_HEADER_BG)
        .inner_margin(egui::Margin::symmetric(8.0, 0.0));

    egui::TopBottomPanel::bottom("status_bar")
        .exact_height(theme::STATUS_BAR_HEIGHT)
        .frame(frame)
        .show(ctx, |ui| {
            // 顶沿 1px 分隔线（panel 的 frame 没有边框可设，直接画一条）。
            ui.painter().hline(
                ui.max_rect().x_range(),
                ui.max_rect().top() + 0.5,
                egui::Stroke::new(1.0_f32, theme::colors::SEPARATOR),
            );

            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;

                // ── 播放态 ──
                // 编辑态用**小圆点**而不是暂停符号：暂停图标在"没在跑"的
                // 语境里读起来像"运行中的暂停"，是状态栏最容易误读的一处。
                let (icon, text, color) = match state.play_mode {
                    PlayMode::Edit => (
                        icons::Icon::Dot,
                        "Edit Mode",
                        theme::colors::TEXT_SECONDARY,
                    ),
                    PlayMode::Playing => (icons::Icon::Play, "Playing", theme::colors::PLAY_GREEN),
                    PlayMode::Paused => (icons::Icon::Pause, "Paused", theme::colors::WARN),
                };
                icons::small_icon(ui, icon, 9.0, color);
                ui.label(egui::RichText::new(text).size(11.0).color(color));
                icons::v_separator(ui);

                // ── 项目 / 场景 ──
                ui.label(
                    egui::RichText::new(state.project_name())
                        .size(11.0)
                        .color(theme::colors::TEXT_PRIMARY),
                );
                ui.label(
                    egui::RichText::new("·")
                        .size(11.0)
                        .color(theme::colors::TEXT_DIM),
                );
                ui.label(
                    egui::RichText::new(&state.scene.name)
                        .size(11.0)
                        .color(theme::colors::TEXT_PRIMARY),
                );
                ui.label(
                    egui::RichText::new(format!("{} entities", state.scene.entities.len()))
                        .size(11.0)
                        .color(theme::colors::TEXT_DIM),
                );

                // ── 物理 / 音频（只在运行时有意义）──
                if state.play_mode != PlayMode::Edit {
                    icons::v_separator(ui);
                    let (bodies, colliders) = state.physics_stats();
                    ui.label(
                        egui::RichText::new(format!("{bodies} bodies / {colliders} colliders"))
                            .size(11.0)
                            .color(theme::colors::TEXT_SECONDARY),
                    );
                    // 音频只显示"在响的"和设备状态（silent = 无声降级，用警告色）。
                    let audio = state.audio_stats();
                    ui.label(
                        egui::RichText::new(format!(
                            "audio {} · {} active",
                            if audio.available { "device" } else { "silent" },
                            audio.active,
                        ))
                        .size(11.0)
                        .color(if audio.available {
                            theme::colors::TEXT_SECONDARY
                        } else {
                            theme::colors::WARN
                        }),
                    );
                    // 粒子同理：活着多少 / 上限多少。上限数字要能看到 ——
                    // "粒子数撞顶"是这个系统最主要的性能故障模式，而它在画面上
                    // 的表现只是"莫名变少 / 不喷了"。
                    let particles = state.particle_stats();
                    if particles.emitters > 0 {
                        icons::v_separator(ui);
                        ui.label(
                            egui::RichText::new(format!(
                                "particles {} / {} ({} emitters)",
                                particles.alive, particles.capacity, particles.emitters
                            ))
                            .size(11.0)
                            .color(if particles.alive >= particles.capacity {
                                theme::colors::WARN
                            } else {
                                theme::colors::TEXT_SECONDARY
                            }),
                        )
                        .on_hover_text(
                            "活着 / 上限（所有发射器之和）。\n撞上限时新粒子不再出生 —— 调大 Max 或调小 Rate",
                        );
                    }
                }

                // ── 右侧：FPS + 版本 ──
                //
                // 读真实帧间隔（`stable_dt`），别用 `predicted_dt` —— 后者是预测值，
                // 帧率真的掉下去时它可能仍然报 60，等于把卡顿藏起来。
                // 空闲（帧调度已停）时不能继续显示上一次的读数 —— 那看起来像界面卡死，
                // 显式标成 `idle`，一眼就知道"不是卡、是它真的闲下来了"。
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("v0.1.0")
                            .size(10.5)
                            .color(theme::colors::TEXT_DIM),
                    );
                    icons::v_separator(ui);
                    let fps = if live {
                        format!("{:.0} fps", ctx.input(|i| i.stable_dt).recip().round())
                    } else {
                        "idle".to_owned()
                    };
                    ui.label(
                        egui::RichText::new(fps)
                            .monospace()
                            .size(11.0)
                            .color(if live {
                                theme::colors::TEXT_PRIMARY
                            } else {
                                theme::colors::TEXT_DIM
                            }),
                    );

                    // ── 自动保存 ──
                    //
                    // 显示的是**上一次成功写入**的相对时间 —— 用户唯一关心的
                    // 就是这个（"我最多会丢多少工作"）。失败优先显示：一条
                    // "自动保存失败"比"3 分钟前"更急。
                    if let Some((text, color)) = autosave_label(state) {
                        icons::v_separator(ui);
                        let interval = state.autosave_state().interval_secs();
                        ui.label(egui::RichText::new(text).size(11.0).color(color))
                            .on_hover_text(format!(
                                "每 {interval} 秒在 {}/ 下留一份副本。\n副本不改「未保存」状态 —— 要真正落盘仍需 Ctrl+S。",
                                xgame_core::project::AUTOSAVE_DIR
                            ));
                    }
                });
            });
        });
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

fn main() -> Result<(), eframe::Error> {
    // 日志后端：stderr（`RUST_LOG` 语义不变）+ 控制台面板 + `<配置目录>/logs/Editor.log`。
    // 见 `logging` 模块头：没有这个，双击 exe 跑出来的崩溃现场一行都留不下。
    logging::init();

    // ── 无头项目检查 / 无头运行 ──
    //
    // 两者都不开窗口、不建 GPU，秒级返回。退出码是给 CI / **AI 智能体**的契约：
    //   --check：0 = 干净 / 1 = 加载失败 / 2 = 能加载但有悬空引用
    //   --headless：0 = 干净跑完 / 1 = 加载失败或参数错误 / 2 = 跑完但有脚本
    //               error 日志或非有限变换
    //   --build：0 = 打包成功 / 1 = 失败（入口缺失 / 发布清单有装不起来的关卡 / 找不到播放器）
    //   --export-schema：0 = 导出成功 / 1 = 参数或序列化错误
    //   --export-script-api：0 = 导出成功 / 1 = 参数或序列化错误
    // 详见 xgame-core `check` 模块头与 `headless` 模块头（验证 / 运行逻辑在
    // 那里，这里只管分发与退出码）。
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        match args.first().map(String::as_str) {
            Some("--export-schema") => {
                std::process::exit(crate::schema_cli::cli(&args[1..]));
            }
            Some("--export-script-api") => {
                std::process::exit(crate::script_api_cli::cli(&args[1..]));
            }
            Some("--check") => {
                let path = args
                    .get(1)
                    .map(String::as_str)
                    .unwrap_or("projects/SampleScene");
                match xgame_core::check_project(Path::new(path)) {
                    Ok(report) => {
                        print!("{}", report.render());
                        std::process::exit(if report.has_issues() { 2 } else { 0 });
                    }
                    Err(e) => {
                        eprintln!("check FAILED: {e}");
                        std::process::exit(1);
                    }
                }
            }
            Some("--migrate-assets") => {
                let path = args
                    .get(1)
                    .map(String::as_str)
                    .unwrap_or("projects/SampleScene");
                match xgame_core::ProjectHandle::open(Path::new(path)) {
                    Ok(project) => {
                        let report = project.migrate_asset_references();
                        print!("{}", report.render());
                        // 迁移是"尽力而为"的批处理：有文件失败才返回 1。
                        std::process::exit(if report.has_errors() { 1 } else { 0 });
                    }
                    Err(e) => {
                        eprintln!("migrate FAILED: {e}");
                        std::process::exit(1);
                    }
                }
            }
            Some("--headless") => {
                let rest = &args[1..];
                // `--help` 在这里截住：`parse_args` 只认真正的选项，帮助文案
                // 不该混进参数解析（"help 也算一个已解析参数"会误导后续维护）。
                if rest.iter().any(|a| a == "--help" || a == "-h") {
                    print!("{}", headless::usage());
                    std::process::exit(0);
                }
                std::process::exit(headless::cli(rest));
            }
            // 构建：与工具栏 Build 按钮**同一个函数**（`builder::build`），
            // 只是不起窗口。给 CI / 智能体一条"打完包再说"的路 —— 不然
            // 构建结果只能靠人点按钮看 Console。
            Some("--build") => {
                let path = args
                    .get(1)
                    .map(String::as_str)
                    .unwrap_or("projects/SampleScene");
                let code = match xgame_core::ProjectHandle::open(Path::new(path)) {
                    Ok(project) => match crate::builder::build(&project) {
                        Ok(report) => {
                            println!("{}", report.summary());
                            for warning in &report.warnings {
                                println!("  warn: {warning}");
                            }
                            println!("scene_count: {}", report.scene_count);
                            0
                        }
                        Err(e) => {
                            // 多行错误（发布清单里一次列全失败项）原样打到 stderr。
                            eprintln!("build FAILED: {e}");
                            1
                        }
                    },
                    Err(e) => {
                        eprintln!("build FAILED: {e}");
                        1
                    }
                };
                std::process::exit(code);
            }
            _ => {}
        }
    }

    // ── 插件临时副本清扫（P0.5）──
    // Windows 上被 dlopen 的 DLL 文件会锁到进程结束，所以加载走的是
    // `%TEMP%/xgame-plugin-cache/` 里的唯一副本；上一个编辑器进程退出后那些
    // 文件就成了垃圾。放在无头分支之后：--check / --headless 都不碰临时副本，
    // 没必要为它们扫盘。
    crate::plugin_build::sweep_temp_copies();

    // 上屏参数 A/B 开关（诊断用）：
    //   XGAME_PRESENT_MODE   fifo|fifo_relaxed|immediate|mailbox|auto_no_vsync|auto_vsync
    //   XGAME_FRAME_LATENCY  1|2|3
    // 不设 = 默认 AutoNoVsync（不等垂直同步，交给 DWM 按屏幕刷新率合成）。
    let present_mode = match std::env::var("XGAME_PRESENT_MODE").as_deref() {
        Ok("fifo") => wgpu::PresentMode::Fifo,
        Ok("fifo_relaxed") => wgpu::PresentMode::FifoRelaxed,
        Ok("immediate") => wgpu::PresentMode::Immediate,
        Ok("mailbox") => wgpu::PresentMode::Mailbox,
        Ok("auto_no_vsync") => wgpu::PresentMode::AutoNoVsync,
        Ok("auto_vsync") => wgpu::PresentMode::AutoVsync,
        _ => wgpu::PresentMode::AutoNoVsync,
    };
    let frame_latency: Option<u32> = std::env::var("XGAME_FRAME_LATENCY")
        .ok()
        .and_then(|s| s.parse().ok());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_title("XGameEngine"),
        // 不限帧上屏（AutoNoVsync → Immediate，不支持时逐级回退到 Fifo）。
        //
        // 这是"界面拖动卡顿/迟钝"的根因修复，实测依据（PresentMon 2.6 + 帧探针，
        // RTX 2080 1440x900，hybrid 策略拖拽 5s）：
        //
        // - vsync（Fifo / FifoRelaxed，latency=1）：交换链 acquire 与 present 的
        //   等待会叠加，update() 节奏变成 33/1/16ms 的三连拍 —— 每 ~50ms 就有一段
        //   33ms 盲区，期间**输入完全不被处理**、界面内容按三连拍刷新（一顿一顿）；
        //   提交→上屏 32.8ms，输入→光子 p90 ≈ 50ms。
        // - vsync + latency=2：帧挤成串、再整段停 33-50ms，比 latency=1 更糟。
        // - 不限帧：update 间隔 p50=0.5ms / p99=1.9ms，无盲区；提交→上屏 17.5ms
        //   （p99 18.9）；屏上节奏由 DWM 保证（动画误差≈0）。交互期代价约 1 核
        //   CPU + ~11% GPU，闲置时 frame_policy 照常停帧（0 占用）。
        //
        // 想回 vsync 对比：XGAME_PRESENT_MODE=fifo（或 fifo_relaxed）。
        // desired_maximum_frame_latency 保持 1：仅对 vsync 模式有意义（A/B 中
        // lat=1 优于 lat=2），对不限帧模式无影响。
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            present_mode,
            desired_maximum_frame_latency: frame_latency.or(Some(1)),
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
            frame(
                &ctx,
                &mut last,
                vec![egui::Event::PointerMoved(p(10.0, 20.0))]
            ),
            1,
            "首次出现的指针位置应当放行"
        );
        // 同一位置再来一次（Windows 的空转来源）→ 丢掉
        assert_eq!(
            frame(
                &ctx,
                &mut last,
                vec![egui::Event::PointerMoved(p(10.0, 20.0))]
            ),
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
            frame(
                &ctx,
                &mut last,
                vec![egui::Event::PointerMoved(p(31.0, 40.0))]
            ),
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

        assert_eq!(
            frame(
                &ctx,
                &mut last,
                vec![egui::Event::PointerMoved(p(5.0, 5.0))]
            ),
            1
        );
        assert_eq!(
            frame(
                &ctx,
                &mut last,
                vec![egui::Event::PointerMoved(p(5.0, 5.0))]
            ),
            0
        );
        assert_eq!(frame(&ctx, &mut last, vec![egui::Event::PointerGone]), 1);
        assert_eq!(
            frame(
                &ctx,
                &mut last,
                vec![egui::Event::PointerMoved(p(5.0, 5.0))]
            ),
            1,
            "离开再回到同一点必须放行，否则悬停会失效"
        );
    }

    /// 原始输入的相对位移（`Event::MouseMoved`，来自 `DeviceEvent::MouseMotion`）
    /// 必须被丢弃。
    ///
    /// 光电鼠标的传感器噪声以 ±1~2 像素的 `MouseMoved` 持续灌进来 ——
    /// 实测指针完全静止时编辑器仍以 ~16fps 出帧，事件日志定位源头就是它。
    /// 它会把帧调度的"刚有输入"宽限期无限续命，让 Hybrid 策略永远停在
    /// "活跃"档。一旦这条过滤失效，空闲出帧就会从"真正归零"退化成
    /// "跟着传感器噪声走"。
    #[test]
    fn raw_mouse_motion_noise_is_dropped() {
        let ctx = egui::Context::default();
        let mut last = None;

        // 传感器噪声的典型形状：±1~2 像素的相对位移。
        assert_eq!(
            frame(
                &ctx,
                &mut last,
                vec![egui::Event::MouseMoved(egui::vec2(0.0, -1.0))]
            ),
            0,
            "原始相对位移应当被丢弃 —— 没人消费它，留着只会续命宽限期"
        );

        // 混在真实指针移动里也一样被丢。
        assert_eq!(
            frame(
                &ctx,
                &mut last,
                vec![
                    egui::Event::PointerMoved(egui::pos2(100.0, 100.0)),
                    egui::Event::MouseMoved(egui::vec2(1.0, 0.0)),
                ]
            ),
            1,
            "只有绝对位置事件该留下"
        );
    }

    /// `--ui-size` 的解析：写错了要**报错**，不能静默退化成缺省尺寸。
    ///
    /// 排版回归靠这个参数逐档扫窗口宽度，参数被吃掉而没人发现的话，
    /// 那一档截图其实还是 1440×900 —— 看过一遍什么都没验证到。
    #[test]
    fn ui_size_parses_and_rejects_garbage() {
        assert_eq!(parse_size("1280x800"), Some(egui::vec2(1280.0, 800.0)));
        assert_eq!(parse_size(" 1920X1080 "), Some(egui::vec2(1920.0, 1080.0)));
        assert_eq!(parse_size("1600*1000"), Some(egui::vec2(1600.0, 1000.0)));
        // 太小画不下四列面板 —— 当参数错误，让调用方报出来。
        assert_eq!(parse_size("320x240"), None);
        assert_eq!(parse_size("1280"), None);
        assert_eq!(parse_size("1280xabc"), None);
        assert_eq!(parse_size(""), None);
    }

    /// 截图开关互不干扰，且项目路径仍然取「第一个非开关参数」。
    #[test]
    fn ui_shot_flags_compose() {
        let raw: Vec<String> = [
            "--ui-shot",
            "a.png",
            "--ui-play",
            "--ui-profiler",
            "--ui-preferences",
            "--ui-command",
            "--ui-restore-prefs",
            "--ui-size",
            "1280x800",
            "--ui-select",
            "Fox",
            "projects/SampleScene",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let args = parse_launch_args(&raw);
        assert_eq!(args.project.as_deref(), Some("projects/SampleScene"));
        let shot = args.ui_shot.expect("应当解析出截图请求");
        assert!(shot.play, "--ui-play 应当置位");
        assert!(shot.profiler, "--ui-profiler 应当置位");
        assert!(
            shot.open_preferences,
            "--ui-preferences 应当置位（否则 Preferences 窗口永远截不到）"
        );
        assert!(
            shot.open_command,
            "--ui-command 应当置位（否则命令面板永远截不到）"
        );
        assert!(args.restore_prefs, "--ui-restore-prefs 应当置位");
        assert_eq!(shot.size, egui::vec2(1280.0, 800.0));
        assert_eq!(shot.select.as_deref(), Some("Fox"));
        assert!(!shot.open_settings);
    }
}
