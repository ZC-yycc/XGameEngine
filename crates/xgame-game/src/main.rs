//! XGameEngine 独立游戏二进制。
//!
//! 在无编辑器的情况下运行一个**游戏项目**：打开项目 → 建 [`World`] 运行时 →
//! 每帧驱动它 → 用共享的 `ScenePipeline` 画出来。
//!
//! 与编辑器「▶ Play」跑的是**同一段** `World::update`（见 `xgame-runtime`），
//! 所以预览和成品不会有"行为不一样"这种事。
//!
//! # 运行
//!
//! ```sh
//! cargo run -p xgame-game -- projects/SampleScene
//! ```
//!
//! # 项目定位顺序
//!
//! 1. **命令行参数**——开发时指定任意项目目录；
//! 2. **可执行文件同目录的 `project.ron`**——编辑器「🔨 Build」产出的游戏就是这个
//!    布局（exe 与项目文件放同一个文件夹），所以打包出去的游戏**双击即玩**；
//! 3. **仓库内的示例项目**——`cargo run` 时的兜底。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use winit::{
    application::ApplicationHandler,
    event::{ElementState, KeyEvent, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{Key, NamedKey, PhysicalKey},
    window::{Window, WindowId},
};

use xgame::prelude::*;

/// 既没传命令行参数、可执行文件旁边也没有项目时，退回的示例项目
/// （相对进程工作目录，只在仓库内 `cargo run` 时有意义）。
const DEFAULT_PROJECT: &str = "projects/SampleScene";

/// 单帧 dt 上限。
///
/// 窗口被拖动、系统挂起之后，两帧之间可能隔了好几秒。不夹住的话运行时
/// 会一口气补上千个固定步，等于卡死 —— 夹到 250 ms 只丢帧不爆炸。
const MAX_FRAME_DT: f32 = 0.25;

/// 可执行文件所在目录。
///
/// 打包出去的游戏把 `project.ron` 与 exe 放在一起，靠这个目录找到自己的项目。
fn player_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// 按「命令行参数 → exe 同目录 → 仓库示例项目」的顺序定位要运行的项目。
fn resolve_project() -> Result<ProjectHandle, String> {
    // 1. 命令行参数优先
    if let Some(arg) = std::env::args().nth(1) {
        return ProjectHandle::open(&arg)
            .map_err(|e| format!("无法打开项目 '{arg}': {e}\n用法: xgame-game [项目路径]"));
    }

    // 2. 可执行文件旁边就有项目清单——编辑器构建出的游戏走这条
    if let Some(dir) = player_dir() {
        if dir.join(PROJECT_FILE_NAME).is_file() {
            return ProjectHandle::open(&dir)
                .map_err(|e| format!("无法打开可执行文件同目录的项目 '{}': {e}", dir.display()));
        }
    }

    // 3. 兜底：仓库内的示例项目
    ProjectHandle::open(DEFAULT_PROJECT).map_err(|e| {
        format!(
            "无法打开项目 '{DEFAULT_PROJECT}': {e}\n\
             用法: xgame-game [项目路径]\n\
             提示: 把 project.ron（连同 Scenes/ Resources/ Scripts/）与可执行文件放在同一目录，\
             即可直接双击运行。"
        )
    })
}

// ---------------------------------------------------------------------------
// 应用状态
// ---------------------------------------------------------------------------

struct GameApp {
    /// 当前运行的项目（窗口标题、资产路径都从它来）。
    project: ProjectHandle,
    /// 运行时世界：场景 + 物理 + 输入 + 脚本。
    world: World,
    /// 已加载的 Rust 插件句柄（`Plugins/*.dll`，P0.5）。
    ///
    /// **必须声明在 `world` 之后**：struct 字段按声明顺序 drop，`world` 里的
    /// 组件实例与注册表全是**指向这些 DLL 的函数指针**，它们得先销毁，否则
    /// drop 胶水会跳进已 FreeLibrary 的代码区。
    ///
    /// 本字段**从不被读**是刻意的：它的全部意义就是"占住句柄直到进程结束"
    /// （Drop 实现也不卸载 —— 详见 `xgame::plugin` 模块文档）。
    #[allow(dead_code)]
    plugins: Vec<xgame::plugin::LoadedPlugin>,
    pipeline: ScenePipeline,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// 场景 pass 的 HDR 附件（颜色 / MSAA / 深度）—— 场景画进浮点中间标，
    /// 高光在色调映射前不被裁剪（见 `xgame_render::postfx` 模块文档）。
    hdr: HdrTargets,
    /// 全屏后处理 pass（曝光 + 色调映射），写交换链。
    postfx: PostFx,
    /// TAA 的历史缓冲与 resolve（`render.taa.enabled` 关着时不用，也不建历史纹理）。
    taa: TaaResolver,
    /// 屏幕空间 UI（后处理之后画到交换链上）。
    ui: UiRenderer,
    /// `hdr` 的**代数**：resize 重建时 +1，作为 `postfx` 绑定组的缓存键
    /// （纹理视图没有稳定身份，不能按指针判等）。
    target_gen: u64,
    /// 主 pass 的采样数（1 = 未开 MSAA，见 `supported_sample_count`）。
    sample_count: u32,
    model_cache: HashMap<String, GltfMesh>,
    /// 每实体的动画播放时钟。用**帧 dt** 推进（游戏循环本来就有），与编辑器
    /// 墙钟推进的 `SceneRenderer` 是同一份逻辑的两个宿主。
    anim_driver: AnimationDriver,
}

impl GameApp {
    async fn new(window: &winit::window::Window, project: ProjectHandle) -> Self {
        // -- wgpu 初始化 --
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::empty(),
            ..Default::default()
        });

        let surface = instance.create_surface(window).unwrap();
        // SAFETY: the window outlives the surface (see `GameRunner` field order).
        let surface: wgpu::Surface<'static> = unsafe { std::mem::transmute(surface) };

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find a GPU adapter");

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("XGameEngine Game Device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("Failed to create wgpu device");

        let surface_caps = surface.get_capabilities(&adapter);
        let size = window.inner_size();
        // 垂直同步跟随项目清单：关掉时用立即呈现模式。
        let present_mode = if project.manifest().window.vsync {
            wgpu::PresentMode::Fifo
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_caps.formats[0],
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // 采样数由设备能力决定：不支持的格式上硬开 4× 会让管线创建失败。
        // **按 HDR 格式问** —— 场景 pass 画进 `Rgba16Float`，采样数是格式相关的量。
        let sample_count = supported_sample_count(&adapter, HDR_FORMAT);
        log::info!("渲染：HDR（{HDR_FORMAT:?}）+ ACES 色调映射，MSAA x{sample_count}");

        let (width, height) = (config.width.max(1), config.height.max(1));
        let hdr = HdrTargets::new(&device, "game", width, height, sample_count);
        let mut pipeline = ScenePipeline::new(&device, HDR_FORMAT, sample_count);
        // 共享材质资产（`.mat`）需要项目句柄才能把项目相对路径解析成绝对路径。
        // 独立游戏只在启动时绑定一次；资产热重载是编辑器的能力。
        pipeline.set_project(Some(project.clone()));
        // 显示目标 = 交换链格式：非 sRGB 的交换链由着色器手动编码（见 postfx）。
        let postfx = PostFx::new(&device, config.format);
        let taa = TaaResolver::new(&device, sample_count);
        let ui = UiRenderer::new(&device, config.format);

        // -- 场景（路径都按项目根解析）--
        // 版本号 / 关卡数是**发布信息**：打包出去的 exe 得能自报"我是哪个版本、
        // 带了几关"，否则拿到一份日志也说不清跑的是哪一版（`--headless` 同理）。
        {
            let manifest = project.manifest();
            log::info!(
                "{} v{} — 启动场景 {}，发布清单 {} 关",
                manifest.window.title,
                manifest.build.version,
                manifest.entry_scene,
                manifest.scene_order().len()
            );
        }
        let entry_scene = project.manifest().entry_scene.clone();
        let scene = match project.load_entry_scene() {
            Ok(scene) => scene,
            Err(e) => {
                log::error!("{e}");
                Scene::new("Untitled")
            }
        };

        // -- 输入映射：项目里放了 `input.ron` 就用它，没有就用默认游戏映射 --
        let input_path = project.root().join(INPUT_MAP_FILE);
        let input_map = match InputMap::load(&input_path) {
            Ok(Some(map)) => {
                log::info!("已加载输入映射 {}", input_path.display());
                map
            }
            Ok(None) => InputMap::default_game(),
            Err(e) => {
                log::warn!(
                    "输入映射 {} 解析失败，改用默认映射: {e}",
                    input_path.display()
                );
                InputMap::default_game()
            }
        };

        // -- Rust 插件（P0.5）--
        // 与编辑器「▶ Play」同一来源：加载项目里的 `Plugins/*.dll`，组件注册进
        // 一份新表交给 World。没有插件目录（纯数据项目）时这一步是空扫描 ——
        // 不该因为"这个项目没写 Rust"就走另一条代码路径。
        //
        // 玩家**直接**从 `Plugins/` 加载（不像编辑器那样要临时副本）：玩家是
        // 独立进程、只读一次、不参与编译，那个文件没有被覆盖的需求。
        let mut behaviors = BehaviorRegistry::new();
        let plugins =
            xgame::plugin::load_plugins_dir(project.root().join("Plugins"), &mut behaviors);
        if !plugins.is_empty() {
            log::info!("已加载 {} 个 Rust 插件", plugins.len());
        }

        // -- 运行时 --
        // 重力 / 固定步长来自 project.ron 的 `runtime` 节，与编辑器 Play 同一
        // 入口（`WorldConfig::from_project`）—— 两端行为永远一致。
        //
        // 音频：这里尝试打开真设备（`World::new` 里是 silent，无卡自动降级）。
        // 监听者不用每帧推：本宿主的相机是固定机位 (5,5,5)→原点，
        // 与 `ListenerPose::default()` 完全一致（引擎开设备时就是这个姿态）。
        //
        // 场景装载器 = 项目句柄本身（`ProjectHandle` 是共享资产索引的轻句柄，
        // clone 不重新扫盘）：脚本 `load_scene()` 靠它把 GUID 引用解码成路径。
        // 没有这一步，运行期切场景会记一条"宿主没接入场景装载器"的错误。
        let world = World::new(
            scene,
            project.root(),
            WorldConfig::from_project(project.manifest()),
        )
        .with_input_map(input_map)
        .with_behavior_registry(behaviors)
        .with_audio_engine(AudioEngine::open(project.root()))
        .with_scene_loader(project.clone())
        .with_current_scene(entry_scene)
        // 关卡顺序来自 `project.ron` 的 `build.scenes`：脚本的 `scene_index()` /
        // `load_scene_at(i)` 都读它。未配置 = 空，脚本只能按路径切。
        .with_scene_order(project.manifest().scene_order());

        Self {
            project,
            world,
            plugins,
            pipeline,
            device,
            queue,
            surface,
            config,
            hdr,
            postfx,
            taa,
            ui,
            target_gen: 0,
            sample_count,
            model_cache: HashMap::new(),
            anim_driver: AnimationDriver::new(),
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);

        // HDR 附件整套重建（颜色 / MSAA / 深度，采样数不变），并发消息给
        // 后处理 pass：绑定组按代数失效重建（源视图换了）。
        self.hdr = HdrTargets::new(&self.device, "game", width, height, self.sample_count);
        self.target_gen = self.target_gen.wrapping_add(1);
        // TAA 的历史跟着目标走：尺寸变了，重投影全部对不上，绑定组里钉着的
        // 颜色 / 深度视图也已经作废。
        self.taa.reset();
    }

    /// 把窗口事件翻译成引擎输入。
    ///
    /// 这一层是**唯一**知道 winit 的地方：`xgame-input` 定义自己的键码，
    /// 翻译集中在这里，换窗口库时改的是这一个函数。
    fn handle_input(&mut self, event: &WindowEvent) {
        let state = self.world.input_mut().state_mut();
        match event {
            WindowEvent::KeyboardInput { event, .. } => {
                let KeyEvent {
                    physical_key,
                    logical_key,
                    state: key_state,
                    repeat,
                    ..
                } = event;
                // 按住不放时系统会持续投递 Pressed —— 交给 InputState 去重，
                // 这里原样投递即可（它自己会识别"重复按下"）。
                let _ = repeat;
                // 只认物理键位：它和键盘布局无关，AZERTY 上按 W 的位置，
                // 绑 `KeyW` 的动作照样生效。
                if let PhysicalKey::Code(code) = physical_key {
                    if let Some(code) = key_from_winit(*code) {
                        state.handle_key(code, *key_state == ElementState::Pressed);
                    }
                }
                // 文本输入（将来做输入框/聊天用）。
                if *key_state == ElementState::Pressed {
                    if let Key::Character(text) = logical_key {
                        state.handle_text(text.as_str());
                    }
                }
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => {
                state.handle_mouse_button(
                    MouseButton::from(*button),
                    *btn_state == ElementState::Pressed,
                );
            }
            WindowEvent::CursorMoved { position, .. } => {
                state.handle_cursor_moved(glam::Vec2::new(position.x as f32, position.y as f32));
            }
            WindowEvent::CursorLeft { .. } => {
                // 指针出了窗口：UI 悬停要清掉（否则按钮会一直亮着）。
                state.handle_cursor_left();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(x, y) => glam::Vec2::new(*x, *y),
                    MouseScrollDelta::PixelDelta(pos) => {
                        glam::Vec2::new(pos.x as f32, pos.y as f32)
                    }
                };
                state.handle_scroll(scroll);
            }
            WindowEvent::Focused(focused) => {
                // 失焦要清掉所有"按住"状态，否则切回来时角色会自己一直走。
                state.set_focused(*focused);
            }
            _ => {}
        }
    }

    fn update(&mut self, dt: f32) {
        // UI 的视口与指针：窗口坐标即物理像素（winit 给的 CursorMoved 就是），
        // 所以这里不需要缩放。编辑器那边则要把 egui 的逻辑点换算成视口像素。
        self.world
            .set_ui_viewport((self.config.width, self.config.height));
        // 指针还没进过窗口时传 `None`：`mouse_pos` 的初值 (0,0) 是个合法位置，
        // 直接传会让左上角的元素一开窗就处于"悬停"。
        let input = self.world.input();
        let pointer = input.state().has_cursor().then(|| input.mouse_pos());
        self.world.set_ui_pointer(pointer);
        self.world.update(dt);
        // 动画时钟跟着**帧时间**走：物理/脚本推进的同一拍，保证动画与游戏逻辑
        // 用的是同一个 dt（掉帧时两边一起慢，不会各走各的）。
        // 吃的是**缩放后**的 dt（`world.frame_dt()`）：脚本 `set_time_scale(0)`
        // 暂停时，动画必须一起停 —— 否则角色冻在原地却还在跑步。
        let anim_dt = self.world.frame_dt();
        self.anim_driver.tick(anim_dt, &self.world.scene().entities);
        for log in self.world.take_logs() {
            emit_script_log(&log);
        }
    }

    fn render(&mut self) {
        // -- Camera --
        //
        // 机位来自场景里的**相机实体**（`Entity::camera`）：脚本每帧推它就能做
        // 跟随 / 摇镜 / 切机位，作者的编辑器 Play 与这里读的是同一个解析入口
        // （`World::camera_view`），不存在"编辑器里对、打出去不对"。
        //
        // 场景里一台相机都没有时退回固定的俯瞰机位 —— 老项目（以及没配相机的
        // 演示场景）不该因为引入相机概念就变成黑屏。
        let aspect = self.config.width as f32 / self.config.height as f32;
        let (view_proj, eye) = match self.world.camera_view() {
            Some(cam) => (cam.view_projection(aspect), cam.position),
            None => {
                let default_eye = Vec3::new(5.0, 5.0, 5.0);
                let view = projection::look_at(default_eye, Vec3::ZERO, Vec3::Y);
                let proj = projection::perspective(60f32.to_radians(), aspect, 0.1, 100.0);
                (proj * view, default_eye)
            }
        };

        let scene = self.world.scene();

        // -- 惰性加载模型（路径按项目根解析）--
        for entity in &scene.entities {
            if !entity.model.is_empty() && !self.model_cache.contains_key(&entity.model) {
                match GltfMesh::load(
                    &self.device,
                    &self.queue,
                    self.project.root(),
                    &entity.model,
                ) {
                    Ok(m) => {
                        log::info!(
                            "Loaded model '{}' ({} primitives)",
                            entity.model,
                            m.primitives.len()
                        );
                        self.model_cache.insert(entity.model.clone(), m);
                    }
                    Err(e) => log::warn!("Failed to load model '{}': {:#}", entity.model, e),
                }
            }
        }

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(e) => {
                log::warn!("Surface error: {e}");
                return;
            }
        };
        let frame_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("game_encoder"),
            });

        // 世界矩阵解算一次，渲染与物理同步共用同一份。
        let world_matrices = scene.world_matrices();

        // UI 布局 + 合批（在 3D 之前算好：`prepare` 只碰 CPU）。
        self.ui.prepare(
            &self.device,
            &self.queue,
            scene,
            (self.config.width, self.config.height),
            self.project.root(),
        );

        // 画面设置的真相源是 project.ron 的 `render` 节 —— 与编辑器视口 /
        // 无头截图吃同一份配置（TAA 的开关与历史权重也在里面）。
        let settings = self.project.manifest().render.sanitized();

        // TAA：本帧的亚像素抖动（关着时投影逐位不变，也不推进序列）。
        let taa_on = settings.taa.enabled;
        let jitter = if taa_on {
            self.taa.begin_frame(self.hdr.size)
        } else {
            self.taa.reset();
            [0.0, 0.0]
        };
        let jitter_ndc = jitter_to_ndc(jitter, self.hdr.size);

        self.pipeline.render(
            &self.device,
            &self.queue,
            &mut encoder,
            &self.hdr.color_view,
            self.hdr.msaa_view.as_ref(),
            &self.hdr.depth_view,
            &scene.entities,
            &world_matrices,
            &self.model_cache,
            &self.anim_driver,
            &view_proj,
            jitter_ndc,
            &eye,
            SceneLighting::new(scene.ambient, &scene.lights),
            settings.sky,
            // 粒子：与编辑器视口同一条 pass（见 `ScenePipeline::render`）。
            self.world.particle_quads(),
        );

        // TAA resolve（开着时）—— 在后处理之前，曝光 / 色调映射作用在收敛后的画面。
        let (source_view, source_gen) = if taa_on {
            self.taa.resolve(
                &self.device,
                &self.queue,
                &mut encoder,
                &self.hdr.color_view,
                &self.hdr.depth_view,
                self.hdr.size,
                &view_proj,
                &eye,
                settings.taa,
            )
        } else {
            (&self.hdr.color_view, self.target_gen)
        };

        // 后处理：HDR 场景图 → 曝光 +（Bloom）→ 色调映射 → 交换链。Bloom 链跟
        // `hdr.size` 走（resize 时附件重建、`target_gen` 自增）。
        self.postfx.render(
            &self.device,
            &self.queue,
            &mut encoder,
            source_view,
            source_gen,
            self.hdr.size,
            &frame_view,
            settings,
        );

        // UI：后处理之后、同一条 encoder —— UI 不参与曝光 / 色调映射。
        self.ui.render(
            &self.device,
            &self.queue,
            &mut encoder,
            &frame_view,
            (self.config.width, self.config.height),
        );

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }
}

/// 项目根目录下输入映射的文件名。
const INPUT_MAP_FILE: &str = "input.ron";

/// 把脚本日志转发到 `log` 宏（最终由 env_logger 打到控制台）。
fn emit_script_log(log: &ScriptLog) {
    let entity = log.entity.map(|id| format!(" [{id}]")).unwrap_or_default();
    match log.level {
        LogLevel::Info => log::info!(target: "script", "{}{entity}: {}", log.script, log.message),
        LogLevel::Warn => log::warn!(target: "script", "{}{entity}: {}", log.script, log.message),
        LogLevel::Error => log::error!(target: "script", "{}{entity}: {}", log.script, log.message),
    }
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

/// winit 0.30 的事件处理入口。
///
/// 为什么不是 `event_loop.run(|event, elwt| ...)`：那个闭包版本从 winit 0.30
/// 起已标记废弃，官方入口是 `run_app` + `ApplicationHandler`；而且窗口也**只能**
/// 通过 `ActiveEventLoop::create_window` 创建，后者仅在回调内部拿得到 ——
/// 所以窗口必须推迟到 `resumed` 里再建。
struct GameRunner {
    /// 窗口属性（标题 / 尺寸 / 可缩放），启动时从项目清单取好。
    attributes: winit::window::WindowAttributes,
    /// 项目句柄。`resumed` 时移交给 `GameApp::new`，所以这里用 `Option` 包一层。
    project: Option<ProjectHandle>,
    /// 主窗口，`resumed` 里创建。
    window: Option<Window>,
    /// 渲染 + 游戏逻辑。
    ///
    /// **字段顺序即析构顺序的反序**：`app` 声明在 `window` 之后 → 先于 `window`
    /// 析构。`GameApp` 内部持有 `wgpu::Surface<'static>`（靠 `transmute` 借来窗口
    /// 的生命周期，见 `GameApp::new` 的 SAFETY 注释），必须保证 surface 先于窗口
    /// 销毁，这个顺序就是为了它。
    app: Option<GameApp>,
    /// 上一帧的时间戳，用来算真实帧间隔。
    last_frame: std::time::Instant,
}

impl ApplicationHandler for GameRunner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // 某些平台会重复投递 `resumed`（例如 Android 从后台回到前台），只处理第一次。
        if self.app.is_none() {
            let window = event_loop.create_window(self.attributes.clone()).unwrap();
            let project = self
                .project
                .take()
                .expect("resumed 在窗口创建前被重复触发，项目句柄已被取走");
            self.app = Some(pollster::block_on(GameApp::new(&window, project)));
            self.window = Some(window);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // 输入先喂给运行时（Esc 退出在下面单独判）。
        if let Some(app) = self.app.as_mut() {
            app.handle_input(&event);
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                // 实际出帧在这里，由下面的 `about_to_wait` 主动排重绘驱动。
                // `resumed` 之前也可能收到一次 RedrawRequested，那时 app 还没建好。
                if let Some(app) = self.app.as_mut() {
                    let now = std::time::Instant::now();
                    let dt = (now - self.last_frame).as_secs_f32().min(MAX_FRAME_DT);
                    self.last_frame = now;
                    app.update(dt);
                    app.render();
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(app) = self.app.as_mut() {
                    app.resize(size.width, size.height);
                }
            }
            // Esc 退出。注意默认输入映射里没有把 Esc 绑成"暂停"（用的是 P），
            // 否则玩家按一次 Esc 就会先暂停再退出，很困惑。
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => event_loop.exit(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // 「鼠标不动画面就停」的根因就在这：winit 在没有新事件时会
        // **阻塞等待**，不会自己连续出帧。主动排一次重绘把它唤醒，
        // 于是形成 AboutToWait → RedrawRequested → AboutToWait 的连续帧。
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

fn main() {
    env_logger::init();

    let project = match resolve_project() {
        Ok(project) => project,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    };

    // -- 窗口设置取自项目清单 --
    let window_config = project.manifest().window.clone();
    let attributes = Window::default_attributes()
        .with_title(window_config.title.clone())
        .with_inner_size(winit::dpi::LogicalSize::new(
            window_config.width,
            window_config.height,
        ))
        .with_resizable(window_config.resizable);

    let event_loop = EventLoop::new().unwrap();

    event_loop
        .run_app(&mut GameRunner {
            attributes,
            project: Some(project),
            window: None,
            app: None,
            last_frame: std::time::Instant::now(),
        })
        .unwrap();
}
