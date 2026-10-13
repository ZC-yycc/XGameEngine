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
    pipeline: ScenePipeline,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,
    depth_texture: wgpu::Texture,
    /// 主 pass 的采样数（1 = 未开 MSAA，见 `supported_sample_count`）。
    sample_count: u32,
    /// 开了 MSAA 时的多采样颜色目标：渲染写它，再 resolve 到交换链。
    /// 交换链的视图本身**不能**是多采样的，所以必须多这一张。
    msaa_color: Option<(wgpu::Texture, wgpu::TextureView)>,
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
        let sample_count =
            supported_sample_count(&adapter, config.format);
        log::info!("渲染：MSAA x{sample_count}");

        let (depth_texture, depth_view) =
            Self::create_depth_texture(&device, size.width.max(1), size.height.max(1), sample_count);
        let msaa_color = Self::create_msaa_color(
            &device,
            config.format,
            size.width.max(1),
            size.height.max(1),
            sample_count,
        );

        let pipeline = ScenePipeline::new(&device, config.format, sample_count);

        // -- 场景（路径都按项目根解析）--
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
                log::warn!("输入映射 {} 解析失败，改用默认映射: {e}", input_path.display());
                InputMap::default_game()
            }
        };

        // -- 运行时 --
        // 重力 / 固定步长来自 project.ron 的 `runtime` 节，与编辑器 Play 同一
        // 入口（`WorldConfig::from_project_runtime`）—— 两端行为永远一致。
        let world = World::new(
            scene,
            project.root(),
            WorldConfig::from_project_runtime(&project.manifest().runtime),
        )
        .with_input_map(input_map);

        Self {
            project,
            world,
            pipeline,
            device,
            queue,
            surface,
            config,
            depth_view,
            depth_texture,
            sample_count,
            msaa_color,
            model_cache: HashMap::new(),
            anim_driver: AnimationDriver::new(),
        }
    }

    /// 建多采样颜色目标（`sample_count == 1` 时返回 `None`）。
    fn create_msaa_color(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        sample_count: u32,
    ) -> Option<(wgpu::Texture, wgpu::TextureView)> {
        if sample_count <= 1 {
            return None;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("game_msaa_color"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format,
            // 只当渲染目标：resolve 之后才是交换链那张，这张不需要被采样。
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Some((texture, view))
    }

    fn create_depth_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        sample_count: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("game_depth"),
            size,
            mip_level_count: 1,
            // 深度附件的采样数必须与颜色附件一致，否则渲染通道校验失败。
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);

        let (tex, view) = Self::create_depth_texture(&self.device, width, height, self.sample_count);
        self.depth_texture = tex;
        self.depth_view = view;
        // 多采样颜色目标也得跟着换尺寸（它的尺寸必须与交换链一致）。
        self.msaa_color = Self::create_msaa_color(
            &self.device,
            self.config.format,
            width,
            height,
            self.sample_count,
        );
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
            WindowEvent::MouseInput { state: btn_state, button, .. } => {
                state.handle_mouse_button(MouseButton::from(*button), *btn_state == ElementState::Pressed);
            }
            WindowEvent::CursorMoved { position, .. } => {
                state.handle_cursor_moved(glam::Vec2::new(
                    position.x as f32,
                    position.y as f32,
                ));
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
        self.world.update(dt);
        // 动画时钟跟着**帧时间**走：物理/脚本推进的同一拍，保证动画与游戏逻辑
        // 用的是同一个 dt（掉帧时两边一起慢，不会各走各的）。
        self.anim_driver
            .tick(dt, &self.world.scene().entities);
        for log in self.world.take_logs() {
            emit_script_log(&log);
        }
    }

    fn render(&mut self) {
        // -- Camera --
        //
        // 还没有"相机实体"这个概念，先用固定的轨道相机看向原点。
        let aspect = self.config.width as f32 / self.config.height as f32;
        let eye = Vec3::new(5.0, 5.0, 5.0);
        let view = projection::look_at(eye, Vec3::ZERO, Vec3::Y);
        let proj = projection::perspective(60f32.to_radians(), aspect, 0.1, 100.0);
        let view_proj = proj * view;

        let scene = self.world.scene();

        // -- 惰性加载模型（路径按项目根解析）--
        for entity in &scene.entities {
            if !entity.model.is_empty() && !self.model_cache.contains_key(&entity.model) {
                match GltfMesh::load(&self.device, &self.queue, self.project.root(), &entity.model)
                {
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

        let mut encoder =
            self.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("game_encoder"),
                });

        // 世界矩阵解算一次，渲染与物理同步共用同一份。
        let world_matrices = scene.world_matrices();

        self.pipeline.render(
            &self.device,
            &self.queue,
            &mut encoder,
            &frame_view,
            self.msaa_color.as_ref().map(|(_, v)| v),
            &self.depth_view,
            &scene.entities,
            &world_matrices,
            &self.model_cache,
            &self.anim_driver,
            &view_proj,
            &eye,
            SceneLighting::new(scene.ambient, &scene.lights),
        );

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }
}

/// 项目根目录下输入映射的文件名。
const INPUT_MAP_FILE: &str = "input.ron";

/// 把脚本日志转发到 `log` 宏（最终由 env_logger 打到控制台）。
fn emit_script_log(log: &ScriptLog) {
    let entity = log
        .entity
        .map(|id| format!(" [{id}]"))
        .unwrap_or_default();
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
