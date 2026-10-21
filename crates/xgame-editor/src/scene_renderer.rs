//! 编辑器 3D 场景渲染器。
//!
//! 将场景实体渲染到离屏 wgpu 纹理，然后将该纹理注册到 egui-wgpu，
//! 供场景视图面板显示。
//!
//! 底层渲染逻辑由 `xgame_render::scene_pipeline::ScenePipeline` 提供，
//! 此模块仅负责离屏纹理管理和 egui 集成。
//!
//! 实体的 `model` 字段是**项目相对路径**，加载前会拼上项目根目录
//! （`project_root`），因此编辑器的工作目录是无关的。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use glam::{Mat4, Vec3};

use xgame_render::animation::AnimationDriver;
use xgame_render::gltf_loader::{Bounds, GltfMesh};
use xgame_render::postfx::{HdrTargets, PostFx, HDR_FORMAT};
use xgame_render::scene_pipeline::{SceneLighting, ScenePipeline, MAX_LIGHTS};
use xgame_render::ui_renderer::UiRenderer;
use xgame_core::{Entity, EntityId, RenderConfig, Scene};

/// 视口"还活着"的判定窗口：距上次 `render` 进入本函数超过这个时间就认为
/// 场景视图已经不出图了（面板被关掉），不再为动画请求帧。
const VIEWPORT_LIVENESS: std::time::Duration = std::time::Duration::from_millis(400);

/// 连续缩放窗口时，重建离屏渲染目标的最小间隔。
///
/// 太小 = 又变成"每帧重建"（一次拖拽 40+ 次纹理分配）；太大 = 视口在拖拽过程中
/// 更新得太迟钝。100ms（约 10Hz）在"跟手"与"开销"之间是个稳妥的折中 ——
/// 拖拽结束的那一帧无论如何都会重建一次，所以最终画质不受影响。
const RESIZE_REBUILD_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// 视口尺寸变更时该做什么（纯函数，便于单测 —— 真实路径需要 GPU 设备，测不了）。
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum ResizeDecision {
    /// 什么都不用做（尺寸非法，或与当前渲染目标一致）。
    Ignore,
    /// 沿用旧渲染目标，本帧不重建。
    Defer,
    /// 按新尺寸重建渲染目标。
    Rebuild,
}

/// 决定一次视口尺寸变更该怎么处理，见 [`ResizeDecision`]。
///
/// - `current` 当前渲染目标的尺寸；`asked` 本帧请求的尺寸。
/// - `pending` 上一帧被推迟的请求尺寸（`Some(asked)` = 请求没变 → 用户停手了）。
/// - `has_target` 是否已经有可用的渲染目标（首帧为假，不能等）。
/// - `since_rebuild` 距上次重建过了多久（`None` = 从没建过）。
pub(crate) fn plan_resize(
    current: (u32, u32),
    pending: Option<(u32, u32)>,
    has_target: bool,
    asked: (u32, u32),
    since_rebuild: Option<std::time::Duration>,
) -> ResizeDecision {
    if asked.0 == 0 || asked.1 == 0 || current == asked {
        return ResizeDecision::Ignore;
    }
    // 请求尺寸与上一帧相同 → 尺寸稳住了（用户松手了）。
    let settled = pending == Some(asked);
    // 距上次重建够久 → 即便还在拖，也让视口更新一次，不至于整段冻住。
    let due = since_rebuild.is_none_or(|d| d >= RESIZE_REBUILD_INTERVAL);

    if !has_target || settled || due {
        ResizeDecision::Rebuild
    } else {
        ResizeDecision::Defer
    }
}

// ---------------------------------------------------------------------------
// SceneRenderer
// ---------------------------------------------------------------------------

pub struct SceneRenderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    /// egui-wgpu renderer for registering native textures.
    egui_renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
    surface_format: wgpu::TextureFormat,

    /// 共享渲染管线。
    pipeline: ScenePipeline,

    /// 当前项目的根目录，用于把 `entity.model`（项目相对路径）解析成绝对路径。
    project_root: PathBuf,

    // Render target (recreated when viewport size changes)
    viewport_size: (u32, u32),
    /// 最终交给 egui 的**单采样**颜色纹理（后处理 pass 的输出，LDR）。
    ///
    /// 3D 场景**不**直接画它 —— 画它的是 `postfx`（色调映射之后）。理由见
    /// `xgame_render::postfx` 模块文档：8 位目标的写入是钳制的，高光必须在
    /// 浮点中间标里先被看见。
    render_view: Option<wgpu::TextureView>,
    /// 当前 LDR 显示纹理（`render_view` 是它的视图）。
    /// 重建时整块换掉：旧的那张移进 `_stale_textures` 多留一代。
    _ldr_texture: Option<wgpu::Texture>,
    /// 场景 pass 的 HDR 附件（颜色 / MSAA / 深度），随视口尺寸重建。
    hdr: Option<HdrTargets>,
    /// 全屏后处理 pass（曝光 + 色调映射），源视图按 [`Self::target_gen`] 缓存绑定组。
    postfx: PostFx,
    /// 屏幕空间 UI（在 `postfx` **之后**画到同一张 LDR 目标上）。
    ///
    /// 排在色调映射之后是硬约束：UI 颜色不该随场景曝光变化，见
    /// `xgame_render::ui_renderer` 的模块文档。
    ui: UiRenderer,
    /// 渲染目标的**代数**：每次 `recreate_targets` 自增，作为 `postfx` 的
    /// 绑定组缓存键（纹理视图没有稳定身份，不能按指针判等，见 `PostFx::render`）。
    target_gen: u64,
    /// 上一帧请求过、但被推迟的视口尺寸（见 [`SceneRenderer::set_viewport_size`]）。
    ///
    /// `Some(尺寸)` 表示"尺寸还在变，这一帧先不重建"。`render` 在它为 `Some` 时
    /// **跳过整段 3D 渲染**，只让调用方把上一帧的纹理拉伸到新矩形上。
    pending_size: Option<(u32, u32)>,
    /// 上一次重建渲染目标的时刻，用于给连续缩放节流。
    last_target_rebuild: Option<std::time::Instant>,
    /// 主 pass 的采样数（1 = 未开 MSAA）。
    sample_count: u32,
    /// Keep old render-target textures alive so egui doesn't dangle.
    _stale_textures: Vec<wgpu::Texture>,
    texture_id: Option<egui::TextureId>,
    /// 上一帧真正渲染时用的输入指纹（见 [`SceneRenderer::scene_signature`]）。
    /// `None` 表示"当前纹理内容不可信，下一帧必须重画"——重建渲染目标后会置回 `None`。
    last_signature: Option<u64>,

    // Model cache (path -> GPU mesh)
    model_cache: HashMap<String, GltfMesh>,
    /// 模型加载失败记录（路径）。防止坏资产每帧重试读盘，也给
    /// [`SceneRenderer::viewport_animating`] 一个"这动画永远起不来"的判据。
    load_failed: HashSet<String>,

    /// 动画播放时钟（每实体一份，见 [`AnimationDriver`]）。
    /// 编辑器编辑模式没有自己的帧时钟，由 [`SceneRenderer::render`] 用墙钟推进。
    anim_driver: AnimationDriver,
    /// 上一次真正进入 [`SceneRenderer::render`] 的时刻。
    /// 视口面板关掉后这个时间会自然过期 → `viewport_animating` 停止请求帧，
    /// 免得"视口藏着一只在播动画的狐狸"把编辑器拖成满速空转。
    viewport_seen: Option<std::time::Instant>,
}

impl SceneRenderer {
    pub fn new(
        device: &Arc<wgpu::Device>,
        queue: &Arc<wgpu::Queue>,
        egui_renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
        surface_format: wgpu::TextureFormat,
        project_root: PathBuf,
        sample_count: u32,
    ) -> Self {
        let device = Arc::clone(device);
        let queue = Arc::clone(queue);

        // 场景管线画进 HDR 中间标（Rgba16Float）；显示目标（surface_format）
        // 由后处理 pass 写。**管线格式与显示格式就此分家** —— 这正是高光
        // 不再在写入时被裁掉的原因（见 `xgame_render::postfx` 模块文档）。
        let pipeline = ScenePipeline::new(&device, HDR_FORMAT, sample_count);
        let postfx = PostFx::new(&device, surface_format);
        let ui = UiRenderer::new(&device, surface_format);

        Self {
            device,
            queue,
            egui_renderer,
            surface_format,
            pipeline,
            project_root,
            viewport_size: (0, 0),
            render_view: None,
            _ldr_texture: None,
            hdr: None,
            postfx,
            ui,
            target_gen: 0,
            pending_size: None,
            last_target_rebuild: None,
            sample_count: sample_count.max(1),
            _stale_textures: Vec::new(),
            texture_id: None,
            last_signature: None,
            model_cache: HashMap::new(),
            load_failed: HashSet::new(),
            anim_driver: AnimationDriver::new(),
            viewport_seen: None,
        }
    }

    // -- Viewport management --

    /// 绑定项目句柄：共享材质资产（`.mat`）与贴图导入设置都从这里读。
    ///
    /// 编辑器在打开项目后调用一次；之后 `ProjectHandle` 内部的资产库会随
    /// 保存 / 刷新更新，不需要重新绑定。
    pub fn set_project(&mut self, project: Option<xgame_core::ProjectHandle>) {
        self.pipeline.set_project(project);
        self.last_signature = None;
    }

    /// 材质 / 贴图内容变了：丢缓存并强制下一帧重画。
    ///
    /// 只影响资产材质，模型网格不动 —— 编辑器保存 `.mat` 或改贴图导入设置时用。
    pub fn invalidate_materials(&mut self) {
        self.pipeline.invalidate_materials();
        self.last_signature = None;
    }

    /// 模型 / 贴图导入设置变了：连模型缓存一起丢。
    ///
    /// `model_scale` / 贴图 sRGB / 最近邻都烘焙在 `GltfMesh` 与 GPU 纹理里，
    /// 只清材质库不够。下一帧会按新设置重新加载（坏路径仍进负缓存）。
    pub fn invalidate_assets(&mut self) {
        self.model_cache.clear();
        self.load_failed.clear();
        self.pipeline.invalidate_materials();
        self.last_signature = None;
    }

    /// 视口尺寸变化时**不立刻**重建渲染目标，而是节流重建。
    ///
    /// 拖窗口边缘时这个函数每帧都会被调一次，而重建意味着新建 4 张纹理
    /// （LDR 显示 / HDR 中间标 / MSAA HDR 颜色 / 深度 —— 后两张按采样数放大，
    /// 最贵），再加上注册一张新的 egui 原生纹理、释放旧的；而且尺寸一变
    /// **输入指纹必然变** → 每帧还要全量重绘一次 3D。实测拖一次边框（约 1.7 秒）
    /// 会重建 **43 次**目标、重绘 43 帧 —— 主线程被这些分配占满，Windows 的模态
    /// 缩放循环抢不到消息线程，表现就是缩放发"黏"。
    ///
    /// 现在的策略：
    /// - **尺寸还没稳定** → 先不重建，本帧跳过 3D（`render` 会直接返回）；
    ///   但**最多等 [`RESIZE_REBUILD_INTERVAL`]**，到点就重建一次并重绘一帧 ——
    ///   于是视口在拖拽过程中仍以约 10Hz 更新，不会像 Unity 那样整段冻住。
    /// - **尺寸与上一帧相同** = 用户停手了 → 立刻按最终尺寸重建、全质量重绘一帧。
    ///
    /// 这样大部分帧只剩 egui 自己的开销，窗口缩放跟手；纹理开销也从"每帧一次"
    /// 降到"每 100ms 一次"。
    pub fn set_viewport_size(&mut self, width: u32, height: u32) {
        let decision = plan_resize(
            self.viewport_size,
            self.pending_size,
            self.render_view.is_some(),
            (width, height),
            self.last_target_rebuild.map(|t| t.elapsed()),
        );

        match decision {
            ResizeDecision::Ignore => {}
            ResizeDecision::Defer => {
                // 尺寸还在变、又没到重建时间：本帧不重建。
                self.pending_size = Some((width, height));
            }
            ResizeDecision::Rebuild => {
                self.viewport_size = (width, height);
                self.pending_size = None;
                self.last_target_rebuild = Some(std::time::Instant::now());
                self.recreate_targets();
            }
        }
    }

    fn recreate_targets(&mut self) {
        // 旧纹理必须先从这里摘掉：下面会注册一张新的原生纹理，并把新 id 存回来。
        //
        // 顺带把旧的 egui 纹理 id 显式释放 —— `egui_wgpu::Renderer` 会为每个注册过的
        // 原生纹理在内部留一条表项（带一个 bind group）。只注册不释放的话，**每改一次
        // 视口尺寸就漏一条**，而窗口缩放期间这个函数是每帧都要跑一次的，拖久了会肉眼
        // 可见地变慢。旧纹理本身不用我们操心：`free_texture` 之后 egui 那边就没人引用
        // 它了，wgpu 也会把仍在飞行中的提交里的资源留到提交完成后才真正销毁。
        if let Some(old_id) = self.texture_id.take() {
            self.egui_renderer.write().free_texture(&old_id);
        }
        self.render_view = None;
        // 上一代的附件进 stale 列表多留一会儿 —— 别在飞行中的提交还没完成时
        // 就被释放（egui 那边也可能还引用着上一帧的 texture id）。
        if let Some(old) = self._ldr_texture.take() {
            self._stale_textures.push(old);
        }
        if let Some(old) = self.hdr.take() {
            self._stale_textures.push(old.color);
            self._stale_textures.extend(old.msaa);
            self._stale_textures.push(old.depth);
        }
        // 尺寸变了，之前那张纹理的内容已经作废，下一帧无论如何都要重画。
        self.last_signature = None;
        // 目标换代：`postfx` 的绑定组按这个计数失效重建（源视图换了）。
        self.target_gen = self.target_gen.wrapping_add(1);

        let (w, h) = self.viewport_size;

        // 显示目标：后处理 pass 的输出、egui 注册的那张（surface_format，
        // 与交换链一致，egui 采样它当普通纹理用）。
        let ldr = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("editor_scene_ldr"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.surface_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let ldr_view = ldr.create_view(&wgpu::TextureViewDescriptor::default());

        // HDR 附件（颜色 / MSAA / 深度）由公共模块建 —— 与截图 / 独立游戏
        // 走同一套纹理描述（采样数不匹配的渲染通道会在提交时直接校验失败，
        // 三份手写描述是漂移的温床）。
        let hdr = HdrTargets::new(&self.device, "editor", w, h, self.sample_count);

        // Register with egui-wgpu（注册的必须是**单采样**那张 —— egui 只认它）
        let tid = self
            .egui_renderer
            .write()
            .register_native_texture(&self.device, &ldr_view, wgpu::FilterMode::Linear);

        self.render_view = Some(ldr_view);
        self.hdr = Some(hdr);
        self._ldr_texture = Some(ldr);
        self.texture_id = Some(tid);

        if self._stale_textures.len() > 24 {
            self._stale_textures.drain(..12);
        }
    }

    pub fn texture_id(&self) -> Option<egui::TextureId> {
        self.texture_id
    }

    /// 最近一帧的绘制统计（合批 / 剔除读数，见 `FrameStats`）。
    ///
    /// 读的是**上一次真正渲染**的结果：静态场景的指纹门控会整帧跳过，
    /// 此时视口里的画面就是上一帧那张，统计也应当停在上一帧 —— 与画面一致
    /// 比"每帧更新一个没人看的数"更重要。
    pub fn frame_stats(&self) -> xgame_render::scene_pipeline::FrameStats {
        self.pipeline.frame_stats()
    }

    /// 某个模型标识的本地包围盒（渲染加载过才有），给 UI 侧拾取用。
    ///
    /// 拿不到网格数据时调用方退回单位立方体 —— 内置图元本来就是单位尺寸。
    pub fn mesh_bounds(&self, model: &str) -> Option<Bounds> {
        self.model_cache.get(model).map(|m| m.bounds)
    }

    /// 某模型的动画剪辑名列表（Inspector 的剪辑下拉框用）。
    ///
    /// 模型还没加载时返回空 —— 面板先显示"未加载"，下一帧 `viewport_animating`
    /// 的预加载把资产就位后自然出现选项。
    pub fn animation_clip_names(&self, model: &str) -> Vec<String> {
        self.model_cache
            .get(model)
            .map(|m| m.animations.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default()
    }

    /// 某模型的骨骼（节点）名列表 —— 去重、跳过匿名 —— Inspector 层栈遮罩下拉用。
    ///
    /// 模型还没加载时返回空（同 [`Self::animation_clip_names`] 的约定：面板先显示
    /// "未加载"，预加载完成后自然出现选项）。
    pub fn bone_names(&self, model: &str) -> Vec<String> {
        let Some(mesh) = self.model_cache.get(model) else {
            return Vec::new();
        };
        let mut names: Vec<String> = Vec::new();
        for node in &mesh.nodes {
            if !node.name.is_empty() && !names.iter().any(|n| n == &node.name) {
                names.push(node.name.clone());
            }
        }
        names
    }

    /// 某实体状态机的**当前状态名**（Inspector 显示"现在站在哪"用）。
    ///
    /// 单剪辑模式 / 还没 tick 过 → `None`（面板只在机器模式下显示这一行）。
    pub fn animation_state(&self, id: EntityId) -> Option<&str> {
        self.anim_driver.state(id)
    }

    /// 视口里是否有动画正在**连续播放**（帧策略的输入之一，见 `FrameInputs::animating`）。
    ///
    /// 三个条件缺一不可：
    ///
    /// 1. 实体配了 `animator` 且 [`is_active`](xgame_core::Animator::is_active)
    ///    （开自动播放；机器模式下是"还有速度为正的状态"，见其文档）；
    /// 2. 模型能加载、**剪辑存在** —— 加载失败或剪辑名写错时返回假，否则一个
    ///    坏资产就能让编辑器永远满速空转（这是本函数存在的首要理由）。
    ///    机器模式下要的是「**任一状态**的剪辑在模型里」：entry 剪辑缺失但
    ///    别的状态有效时必须继续出帧，否则转移永远不被求值，机器卡死；
    ///    反过来整台机器的剪辑全都指错 → 假，守住空转底线；
    /// 3. 视口最近真的在出图（[`Self::render`] 打过卡）—— 面板关着时不请求帧。
    ///
    /// 顺带把动画实体的模型**提前加载**：动画请求帧的判定在渲染之前，
    /// "剪辑存在吗"必须先有资产才能答上来。
    pub fn viewport_animating(&mut self, scene: &Scene) -> bool {
        let mut active = false;
        for entity in &scene.entities {
            let Some(animator) = &entity.animator else {
                continue;
            };
            if !animator.is_active() || entity.model.is_empty() {
                continue;
            }
            if !self.model_cache.contains_key(&entity.model)
                && !self.load_failed.contains(&entity.model)
            {
                match GltfMesh::load(
                    &self.device,
                    &self.queue,
                    &self.project_root,
                    &entity.model,
                ) {
                    Ok(m) => {
                        log::info!("Loaded model '{}' for animation", entity.model);
                        self.model_cache.insert(entity.model.clone(), m);
                    }
                    Err(e) => {
                        log::warn!("Failed to load model '{}' for animation: {:#}", entity.model, e);
                        self.load_failed.insert(entity.model.clone());
                    }
                }
            }
            if let Some(mesh) = self.model_cache.get(&entity.model) {
                if !mesh.has_animation() {
                    continue;
                }
                let clip_ok = match animator.active_machine() {
                    Some(m) => m.states.iter().any(|s| mesh.clip(&s.clip).is_some()),
                    None => mesh.clip(&animator.clip).is_some(),
                };
                // 叠加层：权重 > 0 且剪辑在模型里 → 也要出帧（哪怕基础层冻结，
                // 增量还在动）。权重 0 的叠加层贡献零画面，不为它烧帧。
                let additive_ok = animator
                    .additive
                    .as_ref()
                    .is_some_and(|a| a.weight > 0.0 && mesh.clip(&a.clip).is_some());
                // 层栈：任一层权重 > 0 且剪辑在模型里 → 同理要出帧。
                let layers_ok = animator
                    .layers
                    .iter()
                    .any(|l| l.weight > 0.0 && mesh.clip(&l.clip).is_some());
                if clip_ok || additive_ok || layers_ok {
                    active = true;
                }
            }
        }
        active
            && self
                .viewport_seen
                .is_some_and(|t| t.elapsed() < VIEWPORT_LIVENESS)
    }

    // -- Change detection --

    /// 算一帧离屏渲染的**输入指纹**。
    ///
    /// # 为什么需要它
    ///
    /// 编辑器是**事件驱动重绘**的，而"重绘"的触发频率远高于"画面真的变了"的频率。
    /// 最典型的就是拖动窗口：`egui-winit` 把 `WindowEvent::Moved` 也算作需要重绘的
    /// 事件（0.30 起，见其 `lib.rs` 里 `Moved(_) => repaint: true` 那一支），于是
    /// 窗口每挪一下，eframe 就重画一整帧。可窗口位置变化**并不改变客户区内容**，
    /// 这时重跑一次完整的离屏渲染纯属浪费 —— 更糟的是 Windows 的窗口拖动是在主线程
    /// 的模态移动循环里跑的，渲染会把消息循环饿住，表现就是"拖着拖着自己跟不上手"。
    ///
    /// 把"会影响这张纹理的输入"收成一个哈希，逐帧比对，没变就整段跳过：拖窗口、
    /// 拖停靠面板、鼠标在界面里划过……这些帧里 3D 视图一分钱都不花。
    ///
    /// # 覆盖范围
    ///
    /// 只收**着色器真正读到的东西**：相机矩阵与位置、视口尺寸、环境光、参与光照的
    /// 光源、画面设置（曝光 / 色调映射），以及每个实体的模型标识、**局部变换**与
    /// **父引用**。选中态、gizmo、网格开关、物理调试线都不进来 —— 它们画在 egui
    /// 叠加层上，与这张离屏纹理无关。
    ///
    /// # 为什么收「局部变换 + 父引用」，而不是直接收世界矩阵
    ///
    /// 着色器读的是**世界矩阵**，直觉上该直接哈希世界矩阵。但世界矩阵是
    /// `(局部变换, 层级)` 的**纯函数** —— 两者一样，世界矩阵必然一样。所以按
    /// (局部变换 + 父引用) 哈希是**等价**的，却有实打实的好处：
    ///
    /// - 不必为了算指纹先跑一遍 `Scene::world_matrices()`。那个函数每帧都要分配
    ///   几条长度为 n 的 `Vec` 并做 n 次矩阵连乘；在 2000 实体的场景里，这是编辑器
    ///   每一帧（哪怕什么都没动）都要付出的固定成本。现在静态帧的指纹是**零分配、
    ///   零矩阵运算**的一次线性扫描。
    /// - 世界矩阵要 16 个浮点，局部变换只要 10 个 —— 哈希本身也便宜。
    ///
    /// **父引用必须一起哈希**：只哈希局部变换的话，"把子节点挂到另一个父节点下"
    /// 会漏掉（子节点自己的局部变换一个字节都没变，但它的世界矩阵变了）。
    ///
    /// 比对用**位模式**而不是浮点相等：`-0.0`/`NaN` 这类值按位比才是确定、稳定的，
    /// 而这里要的正是"和上一帧逐位相同"，多渲染一帧无害，少渲染一帧才是 bug。
    pub(crate) fn scene_signature(
        viewport: (u32, u32),
        view_proj: &Mat4,
        camera_pos: &Vec3,
        lighting: &SceneLighting<'_>,
        entities: &[Entity],
        anim: u64,
        render: RenderConfig,
    ) -> u64 {
        let mut hash = 0;

        hash = mix_bits(hash, viewport.0 as u64);
        hash = mix_bits(hash, viewport.1 as u64);
        // 动画时钟指纹：有剪辑在播时它**每帧都变**，于是"时间在走"必然绕过
        // 指纹门控、逐帧重画；时钟为空/冻结时是稳定值，静态帧照旧整段跳过。
        hash = mix_bits(hash, anim);
        hash = mix_f32s(hash, &view_proj.to_cols_array());
        hash = mix_f32s(hash, &[camera_pos.x, camera_pos.y, camera_pos.z]);
        hash = mix_f32s(hash, &lighting.ambient);
        // 画面设置（曝光 / 色调映射 / 泛光）进指纹：它们是后处理 pass 的输入，
        // 决定每个像素的最终颜色 —— 漏掉的话"在 Project Settings 里把曝光
        // 翻倍"会看不到视口有任何反应（材质字段曾踩过同一类坑，见本函数文档）。
        hash = mix_bits(hash, render.exposure.to_bits() as u64);
        hash = mix_bits(hash, render.tone_map as u64);
        // FXAA 是后处理链的最后一趟：开关决定目标像素是不是"再过一遍边缘
        // 平滑"，漏进指纹的话在 ⚙ 里勾选它视口会纹丝不动。
        hash = mix_bits(hash, render.fxaa.enabled as u64);
        hash = mix_bits(hash, render.bloom.intensity.to_bits() as u64);
        hash = mix_bits(hash, render.bloom.threshold.to_bits() as u64);
        hash = mix_bits(hash, render.bloom.knee.to_bits() as u64);
        // 天空 / IBL 全字段进指纹：开着天空时它决定背景与所有物体的环境光，
        // 漏一个字段就是"在 ⚙ 里调了天空颜色，视口纹丝不动"（材质/画面设置
        // 都踩过同一类坑）。太阳方向不在这里 —— 它来自灯光，上方灯光段已哈希。
        hash = mix_bits(hash, render.sky.enabled as u64);
        hash = mix_bits(hash, render.sky.intensity.to_bits() as u64);
        hash = mix_f32s(hash, &render.sky.zenith);
        hash = mix_f32s(hash, &render.sky.horizon);
        hash = mix_f32s(hash, &render.sky.ground);
        hash = mix_bits(hash, render.sky.sun_size.to_bits() as u64);
        hash = mix_bits(hash, render.sky.sun_intensity.to_bits() as u64);

        // 光源按渲染管线同样的规则取（只算 active、最多 MAX_LIGHTS 盏），
        // 免得一个超出上限的光源改了参数却白触发一次重绘。
        let mut light_count = 0u64;
        for light in lighting
            .lights
            .iter()
            .filter(|l| l.is_active())
            .take(MAX_LIGHTS)
        {
            light_count += 1;
            hash = mix_bits(hash, light.kind as u64);
            hash = mix_f32s(hash, &light.color);
            hash = mix_bits(hash, light.intensity.to_bits() as u64);
            hash = mix_f32s(hash, &light.position);
            hash = mix_f32s(hash, &light.normalized_direction());
            // 阴影开关与分辨率也进指纹：它们决定深度图的内容/尺寸，
            // 漏掉的话"在 Inspector 里勾掉 Casts Shadow"会看到画面纹丝不动。
            hash = mix_bits(hash, light.casts_shadow as u64);
            hash = mix_bits(hash, light.shadow_resolution as u64);
        }
        hash = mix_bits(hash, light_count);

        for entity in entities.iter() {
            // 与管线保持一致：没有模型的实体根本不会被画出来。
            if entity.model.is_empty() {
                continue;
            }
            // 模型标识是字符串，逐字节搅进去就够 —— 路径都很短，不差这点开销。
            for byte in entity.model.as_bytes() {
                hash = mix_bits(hash, *byte as u64);
            }
            // 层级：父引用（`EntityId` 非零，所以 0 是安全的"无父"哨兵）。
            hash = mix_bits(hash, entity.parent.map_or(0, EntityId::raw));
            // 局部变换 —— 加上父引用就等价于世界矩阵（见上面的说明）。
            hash = mix_f32s(hash, &entity.transform.position.to_array());
            hash = mix_f32s(hash, &entity.transform.rotation.to_array());
            hash = mix_f32s(hash, &entity.transform.scale.to_array());
            // 动画配置：换剪辑/改速度/开关循环与改变换同权 —— 暂停状态下在
            // Inspector 里换剪辑也必须触发一次重绘，否则画面停在旧姿势。
            if let Some(animator) = &entity.animator {
                for byte in animator.clip.as_bytes() {
                    hash = mix_bits(hash, *byte as u64);
                }
                hash = mix_bits(hash, animator.speed.to_bits() as u64);
                hash = mix_bits(hash, animator.looping as u64);
                hash = mix_bits(hash, animator.autoplay as u64);
                hash = mix_bits(hash, animator.fade_time.to_bits() as u64);
                // 状态机配置同样进指纹：**参数值**变了（脚本每帧推、Inspector
                // DragValue 拖）要触发重绘，否则转移换的状态不显示；states/
                // transitions/entry 改了同理（编辑期改 RON 后重载场景才画对）。
                if let Some(m) = animator.active_machine() {                    for byte in m.entry.as_bytes() {
                        hash = mix_bits(hash, *byte as u64);
                    }
                    // BTreeMap 天然有序 → 参数表哈希稳定，与插入顺序无关。
                    for (name, value) in &m.parameters {
                        for byte in name.as_bytes() {
                            hash = mix_bits(hash, *byte as u64);
                        }
                        hash = mix_bits(hash, value.to_bits() as u64);
                    }
                    for s in &m.states {
                        for byte in s.name.as_bytes() {
                            hash = mix_bits(hash, *byte as u64);
                        }
                        for byte in s.clip.as_bytes() {
                            hash = mix_bits(hash, *byte as u64);
                        }
                        hash = mix_bits(hash, s.speed.to_bits() as u64);
                        hash = mix_bits(hash, s.looping as u64);
                    }
                    for t in &m.transitions {
                        for byte in t.from.as_bytes() {
                            hash = mix_bits(hash, *byte as u64);
                        }
                        for byte in t.to.as_bytes() {
                            hash = mix_bits(hash, *byte as u64);
                        }
                        match &t.when {
                            xgame_core::AnimCondition::AtLeast { param, value } => {
                                hash = mix_bits(hash, 0);
                                for byte in param.as_bytes() {
                                    hash = mix_bits(hash, *byte as u64);
                                }
                                hash = mix_bits(hash, value.to_bits() as u64);
                            }
                            xgame_core::AnimCondition::Below { param, value } => {
                                hash = mix_bits(hash, 1);
                                for byte in param.as_bytes() {
                                    hash = mix_bits(hash, *byte as u64);
                                }
                                hash = mix_bits(hash, value.to_bits() as u64);
                            }
                        }
                    }
                }
                // 叠加层：clip/weight/speed/looping 进指纹 —— 脚本每帧推权重、
                // Inspector 拖权重都要触发重绘，否则增量效果改了画面不动。
                if let Some(add) = &animator.additive {
                    for byte in add.clip.as_bytes() {
                        hash = mix_bits(hash, *byte as u64);
                    }
                    hash = mix_bits(hash, add.weight.to_bits() as u64);
                    hash = mix_bits(hash, add.speed.to_bits() as u64);
                    hash = mix_bits(hash, add.looping as u64);
                }
                // 层栈：每层的 名字/模式/剪辑/权重/速度/循环/遮罩 全进指纹 ——
                // 脚本按名推权重、Inspector 加层/改遮罩都要触发重绘。分隔字节
                // 防不同切分的相邻字符串撞出同一串哈希（["ab","c"] vs ["a","bc"]）。
                for l in &animator.layers {
                    for byte in l.name.as_bytes() {
                        hash = mix_bits(hash, *byte as u64);
                    }
                    hash = mix_bits(hash, l.mode as u64);
                    for byte in l.clip.as_bytes() {
                        hash = mix_bits(hash, *byte as u64);
                    }
                    hash = mix_bits(hash, l.weight.to_bits() as u64);
                    hash = mix_bits(hash, l.speed.to_bits() as u64);
                    hash = mix_bits(hash, l.looping as u64);
                    for m in &l.mask {
                        for byte in m.as_bytes() {
                            hash = mix_bits(hash, *byte as u64);
                        }
                        hash = mix_bits(hash, 0xFF);
                    }
                    hash = mix_bits(hash, 0xFE);
                }
            }
            // 材质资产引用：换 `.mat` 必须重画。资产**内容**的变化不在这里，
            // 由 `render` 把 `pipeline.material_revision()` 混进指纹（编辑器保存
            // 材质后自增），否则改文件内容不会触发重绘。
            match &entity.material_asset {
                Some(path) => {
                    hash = mix_bits(hash, 8);
                    for byte in path.as_bytes() {
                        hash = mix_bits(hash, *byte as u64);
                    }
                }
                None => hash = mix_bits(hash, 9),
            }
            // 材质覆盖：颜色 / 金属度 / 粗糙度决定像素颜色，必须进指纹 ——
            // 漏掉的话"在 Inspector 里调颜色"会看到视口纹丝不动（只在切镜头
            // 或下一帧动画时钟变化时才补画一次）。
            if let Some(m) = &entity.material {
                // 每字段先混一个"是否存在"标记：Some(1.0) 与 None 必须区分，
                // 且 [1.0,1.0,1.0,1.0] 这种"看起来没改"的值也要区分（它意味着
                // 字段被显式覆盖了，将来语义若变不受影响）。
                match m.base_color {
                    Some(c) => {
                        hash = mix_bits(hash, 1);
                        hash = mix_f32s(hash, &c);
                    }
                    None => hash = mix_bits(hash, 2),
                }
                match m.metallic {
                    Some(v) => {
                        hash = mix_bits(hash, 3);
                        hash = mix_bits(hash, v.to_bits() as u64);
                    }
                    None => hash = mix_bits(hash, 4),
                }
                match m.roughness {
                    Some(v) => {
                        hash = mix_bits(hash, 5);
                        hash = mix_bits(hash, v.to_bits() as u64);
                    }
                    None => hash = mix_bits(hash, 6),
                }
            } else {
                hash = mix_bits(hash, 7);
            }
        }

        hash
    }

    // -- Rendering --

    /// 渲染一帧离屏场景。
    ///
    /// 返回本帧**新加载模型的本地包围盒**（调用方回传给 UI 侧做拾取）；
    /// 输入指纹与上一帧逐位相同时整段渲染被跳过，返回空 Vec —— 上一帧的纹理
    /// 仍然有效，调用方照常拿 [`texture_id`](Self::texture_id) 用就行。
    ///
    /// 世界矩阵在指纹比对**之后**才算：静态帧连 `world_matrices()` 这次 O(n) 解算
    /// 都省掉，真正"什么都不做"。指纹里已含的「局部变换 + 父引用」与这份矩阵天然一致。
    ///
    /// `settings` 是清单里的画面设置（曝光 / 色调映射）—— 与独立游戏、无头
    /// 截图同源，视口不存在自己的"调试用"渲染参数。
    pub fn render(
        &mut self,
        scene: &Scene,
        view_proj: &Mat4,
        camera_pos: &Vec3,
        lighting: SceneLighting<'_>,
        settings: RenderConfig,
    ) -> Vec<(String, Bounds)> {
        let (w, h) = self.viewport_size;
        if w == 0 || h == 0 {
            return Vec::new();
        }
        // 只要带着有效视口进来一次就算"活着"——即使本帧被指纹门控跳过。
        // `viewport_animating` 靠这个时间戳判断视口面板是否还在出图。
        self.viewport_seen = Some(std::time::Instant::now());

        // 缩放窗口进行中：跳过整段 3D 渲染。调用方会拿上一帧那张纹理拉伸到新矩形 ——
        // 观感与 Unity 一致，代价是这一帧几乎为零（见 `set_viewport_size`）。
        if self.pending_size.is_some() {
            return Vec::new();
        }

        // 动画时钟**先于指纹**推进：有剪辑在播时时间每帧都变，指纹跟着变，
        // 于是门控放行、下一帧继续来；没动画/已暂停时时间冻结，指纹稳定，
        // 静态场景照旧"没动就不画"。时钟只在真的走到这一步时才 tick ——
        // 面板关掉（render 不再被调）时间自然停走，重开接着播。
        self.anim_driver.tick_wall(&scene.entities);
        let anim_hash = self.anim_driver.signature();

        // 指纹门控的兜底：调用方（`render_scene`）本帧输入处理完才调到这里，指纹
        // 由这里自己算并比对。指纹比对是零分配线性扫描，漏拦一次就是一整帧 GPU 白画。
        let signature = Self::scene_signature(
            self.viewport_size,
            view_proj,
            camera_pos,
            &lighting,
            &scene.entities,
            anim_hash,
            settings,
        );
        // 材质库失效代数进指纹：编辑器保存 `.mat` / 改导入设置后 `invalidate_*`
        // 自增，即使实体字段一个都没变也必须重画（贴图与参数都换了）。
        let signature = mix_bits(signature, self.pipeline.material_revision());
        // UI 的内容签名必须进指纹：否则"场景没动、只有 UI 在变"（按钮悬停
        // 变色、血量数字跳动）会被整帧跳过，看起来像 UI 卡住了。
        let ui_signature = self.ui.prepare(
            &self.device,
            &self.queue,
            scene,
            self.viewport_size,
            &self.project_root,
        );
        let signature = mix_bits(signature, ui_signature);
        if self.last_signature == Some(signature) {
            return Vec::new();
        }

        // 真的要画这一帧了，才解算世界矩阵（O(n) 矩阵乘，静态帧到不了这里）。
        let world_matrices = scene.world_matrices();

        // Phase 1: ensure all models are loaded (mutable borrow on model_cache)
        //
        // 放在指纹判断**之后**是有意的：模型标识本身就进了指纹，所以新加的实体、
        // 改过路径的实体一定会走到这里，不会漏加载；而指纹没变的帧连一次表查找都
        // 不做，更不会去反复读一个坏路径的磁盘文件 —— 那正是"静态界面也卡"的经典成因。
        let mut loaded = Vec::new();
        for entity in &scene.entities {
            if !entity.model.is_empty() && !self.model_cache.contains_key(&entity.model) {
                // entity.model 是项目相对路径（或内置图元标识）；缓存键保持原样不变，
                // 解析与分派都交给 GltfMesh::load。
                match GltfMesh::load(&self.device, &self.queue, &self.project_root, &entity.model) {
                    Ok(m) => {
                        log::info!(
                            "Loaded model '{}' ({} primitives)",
                            entity.model,
                            m.primitives.len()
                        );
                        loaded.push((entity.model.clone(), m.bounds));
                        self.model_cache.insert(entity.model.clone(), m);
                    }
                    Err(e) => {
                        log::warn!("Failed to load model '{}': {:#}", entity.model, e);
                        self.load_failed.insert(entity.model.clone());
                    }
                }
            }
        }

        // Phase 2: render via shared pipeline
        let Some(hdr) = self.hdr.as_ref() else {
            return Vec::new();
        };
        let Some(ldr_view) = self.render_view.as_ref() else {
            return Vec::new();
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("editor_scene_encoder"),
            });

        self.pipeline.render(
            &self.device,
            &self.queue,
            &mut encoder,
            &hdr.color_view,
            hdr.msaa_view.as_ref(),
            &hdr.depth_view,
            &scene.entities,
            &world_matrices,
            &self.model_cache,
            &self.anim_driver,
            view_proj,
            camera_pos,
            lighting,
            settings.sky,
        );

        // 后处理：HDR 场景图 → 曝光 +（Bloom）→ 色调映射 → egui 注册的 LDR 显示
        // 目标。绑定组按目标代数缓存 —— 静态帧整段被指纹拦掉，走到这里的每帧
        // 都要复用同一批 GPU 资源，不能每帧新建绑定组。Bloom 链的尺寸判据用
        // `hdr.size`（附件实际尺寸）而不是请求尺寸：源视图就是它。
        self.postfx.render(
            &self.device,
            &self.queue,
            &mut encoder,
            &hdr.color_view,
            self.target_gen,
            hdr.size,
            ldr_view,
            settings,
        );

        // UI：后处理之后、同一条 encoder。没有 UI 时 `render` 直接返回 false，
        // 一个 pass 都不记（空场景零开销）。
        self.ui
            .render(&self.device, &self.queue, &mut encoder, ldr_view, hdr.size);

        self.queue.submit(std::iter::once(encoder.finish()));

        // 只有真画了才记账 —— 半路 return 的那些分支都没产出可用图像。
        self.last_signature = Some(signature);
        loaded
    }
}

/// 把一个 64 位值搅进哈希（splitmix64 的收尾混合）。
///
/// 自己写而不引第三方哈希库：这里只要"稳定 + 雪崩"，不需要抗碰撞，多一个依赖不划算。
pub(crate) fn mix_bits(hash: u64, value: u64) -> u64 {
    let mut x = hash ^ value.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    x
}

/// 把一串 f32 按**位模式**搅进哈希。
pub(crate) fn mix_f32s(hash: u64, values: &[f32]) -> u64 {
    values
        .iter()
        .fold(hash, |h, v| mix_bits(h, v.to_bits() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgame_core::Light;

    /// 造一个"会被画出来"的实体（有模型标识 = 管线不会跳过它）。
    fn entity(name: &str, model: &str, position: Vec3) -> Entity {
        let mut e = Entity::new(name);
        e.model = model.to_owned();
        e.transform.position = position;
        e
    }

    /// 固定视口与相机，只让调用方关心实体/光源/环境光 —— 测的是"什么该进指纹"。
    /// 测试里的实体都没有父节点，世界矩阵即局部矩阵。
    fn signature(entities: &[Entity], lights: &[Light], ambient: [f32; 3]) -> u64 {
        SceneRenderer::scene_signature(
            (800, 600),
            &Mat4::IDENTITY,
            &Vec3::new(5.0, 5.0, 5.0),
            &SceneLighting::new(ambient, lights),
            entities,
            0,
            RenderConfig::default(),
        )
    }

    // -- 缩放节流状态机 --

    const D: std::time::Duration = std::time::Duration::from_millis(200);

    /// 首帧：还没有渲染目标，必须立刻建，不能等。
    #[test]
    fn first_resize_builds_immediately() {
        assert_eq!(
            plan_resize((0, 0), None, false, (800, 600), None),
            ResizeDecision::Rebuild
        );
    }

    /// 尺寸没变 / 非法尺寸：什么也不做。
    #[test]
    fn unchanged_or_invalid_size_is_ignored() {
        assert_eq!(
            plan_resize((800, 600), None, true, (800, 600), Some(D)),
            ResizeDecision::Ignore
        );
        assert_eq!(
            plan_resize((800, 600), None, true, (0, 600), Some(D)),
            ResizeDecision::Ignore
        );
    }

    /// 连续拖拽中被推迟的尺寸：既没稳定、又刚重建过 → 本帧不重建。
    #[test]
    fn continuous_resize_is_deferred_within_the_throttle_window() {
        assert_eq!(
            plan_resize(
                (800, 600),
                Some((810, 610)),
                true,
                (820, 620),
                Some(std::time::Duration::ZERO)
            ),
            ResizeDecision::Defer
        );
    }

    /// 尺寸与上一帧相同 = 用户松手了 → 立刻按最终尺寸重建（否则画面会永远停在拉伸态）。
    #[test]
    fn settled_size_builds_immediately() {
        assert_eq!(
            plan_resize((800, 600), Some((820, 620)), true, (820, 620), Some(D)),
            ResizeDecision::Rebuild
        );
    }

    /// 还在拖，但距上次重建已经超过节流间隔 → 也重建一次，
    /// 让视口在拖拽过程中保持约 10Hz 的更新，而不是整段冻住。
    #[test]
    fn slow_resize_still_refreshes_after_the_interval() {
        assert_eq!(
            plan_resize(
                (800, 600),
                Some((810, 610)),
                true,
                (820, 620),
                Some(RESIZE_REBUILD_INTERVAL + std::time::Duration::from_millis(1))
            ),
            ResizeDecision::Rebuild
        );
    }

    /// 同一份输入必须给出同一个指纹 —— 否则每帧都会白重画一次。
    #[test]
    fn signature_is_stable_for_identical_input() {
        let e = [entity("Box", "builtin://cube", Vec3::ZERO)];
        let lights = [Light::default()];
        let a = signature(&e, &lights, [0.06, 0.06, 0.08]);
        let b = signature(&e, &lights, [0.06, 0.06, 0.08]);
        assert_eq!(a, b);
    }

    /// 这六样里任何一样变了都必须让指纹变 —— 它们是着色器真正读到的东西。
    /// 少认一样，画面上就是"改了不动"的脏帧。
    #[test]
    fn signature_notices_everything_the_shader_reads() {
        let base = [entity("Box", "builtin://cube", Vec3::ZERO)];
        let lights = [Light::default()];
        let ambient = [0.06, 0.06, 0.08];
        let reference = signature(&base, &lights, ambient);

        // 1. 变换
        let moved = [entity("Box", "builtin://cube", Vec3::new(1.0, 0.0, 0.0))];
        assert_ne!(reference, signature(&moved, &lights, ambient), "变换没进指纹");

        // 2. 旋转 / 缩放（同一位置，只有这两个分量变）
        let mut rotated = entity("Box", "builtin://cube", Vec3::ZERO);
        rotated.transform.rotation = xgame_math::glam::Quat::from_rotation_y(0.5);
        assert_ne!(
            reference,
            signature(&[rotated], &lights, ambient),
            "旋转没进指纹"
        );
        let mut scaled = entity("Box", "builtin://cube", Vec3::ZERO);
        scaled.transform.scale = Vec3::new(2.0, 1.0, 1.0);
        assert_ne!(
            reference,
            signature(&[scaled], &lights, ambient),
            "缩放没进指纹"
        );

        // 3. 模型标识
        let other_model = [entity("Box", "builtin://sphere", Vec3::ZERO)];
        assert_ne!(
            reference,
            signature(&other_model, &lights, ambient),
            "模型标识没进指纹"
        );

        // 4. 光源
        let dim = [Light::directional("sun", [0.5, -1.0, 0.5], [1.0; 3], 1.0)];
        assert_ne!(reference, signature(&base, &dim, ambient), "光源没进指纹");

        // 5. 环境光
        assert_ne!(
            reference,
            signature(&base, &lights, [0.5, 0.5, 0.5]),
            "环境光没进指纹"
        );

        // 6. 相机（换一个 view_proj）
        let moved_camera = SceneRenderer::scene_signature(
            (800, 600),
            &Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0)),
            &Vec3::new(5.0, 5.0, 5.0),
            &SceneLighting::new(ambient, &lights),
            &base,
            0,
            RenderConfig::default(),
        );
        assert_ne!(reference, moved_camera, "相机矩阵没进指纹");

        // 7. 视口尺寸
        let resized = SceneRenderer::scene_signature(
            (1024, 600),
            &Mat4::IDENTITY,
            &Vec3::new(5.0, 5.0, 5.0),
            &SceneLighting::new(ambient, &lights),
            &base,
            0,
            RenderConfig::default(),
        );
        assert_ne!(reference, resized, "视口尺寸没进指纹");

        // 8. 动画时钟（有剪辑在播时它每帧都变 → 必须绕过门控逐帧重画）
        let ticking = SceneRenderer::scene_signature(
            (800, 600),
            &Mat4::IDENTITY,
            &Vec3::new(5.0, 5.0, 5.0),
            &SceneLighting::new(ambient, &lights),
            &base,
            0xDEAD_BEEF,
            RenderConfig::default(),
        );
        assert_ne!(reference, ticking, "动画时钟没进指纹");
    }

    /// 暂停状态下改动画配置（换剪辑）也必须触发重绘 —— 否则 Inspector 里
    /// 换完剪辑画面纹丝不动，用户只能靠挪相机"刷新"。
    #[test]
    fn signature_tracks_animator_config() {
        let lights = [Light::default()];
        let ambient = [0.06, 0.06, 0.08];

        let mut fox = entity("Fox", "Resources/Fox.gltf", Vec3::ZERO);
        fox.animator = Some(xgame_core::Animator::new("Walk"));
        let walking = signature(std::slice::from_ref(&fox), &lights, ambient);

        fox.animator.as_mut().unwrap().clip = "Run".into();
        let running = signature(std::slice::from_ref(&fox), &lights, ambient);
        assert_ne!(walking, running, "换剪辑没进指纹");

        fox.animator.as_mut().unwrap().autoplay = false;
        let paused = signature(std::slice::from_ref(&fox), &lights, ambient);
        assert_ne!(running, paused, "开关自动播放没进指纹");

        fox.animator.as_mut().unwrap().fade_time = 0.0;
        let hard = signature(std::slice::from_ref(&fox), &lights, ambient);
        assert_ne!(paused, hard, "改淡入时长没进指纹");
    }

    /// 暂停状态下改材质也必须触发重绘 —— 与动画配置同一条理由：视口读的是
    /// 指纹门控后的缓存帧，材质没进指纹就"调色没反应"。
    #[test]
    fn signature_tracks_material_override() {
        let lights = [Light::default()];
        let ambient = [0.06, 0.06, 0.08];

        let mut cube = entity("Crate", "builtin://cube", Vec3::ZERO);
        let plain = signature(std::slice::from_ref(&cube), &lights, ambient);

        cube.material = Some(xgame_core::MaterialOverride::default());
        let overridden = signature(std::slice::from_ref(&cube), &lights, ambient);
        assert_ne!(plain, overridden, "打上材质覆盖没进指纹");

        cube.material.as_mut().unwrap().base_color = Some([0.8, 0.2, 0.1, 1.0]);
        let tinted = signature(std::slice::from_ref(&cube), &lights, ambient);
        assert_ne!(overridden, tinted, "改颜色没进指纹");

        cube.material.as_mut().unwrap().metallic = Some(1.0);
        let metal = signature(std::slice::from_ref(&cube), &lights, ambient);
        assert_ne!(tinted, metal, "改金属度没进指纹");

        cube.material.as_mut().unwrap().roughness = Some(0.1);
        let glossy = signature(std::slice::from_ref(&cube), &lights, ambient);
        assert_ne!(metal, glossy, "改粗糙度没进指纹");

        // 清掉覆盖 → 指纹必须回到"从来没有覆盖"的原值（同一场景内容）。
        cube.material = None;
        assert_eq!(
            plain,
            signature(std::slice::from_ref(&cube), &lights, ambient),
            "清掉覆盖后指纹应回到原值"
        );
    }

    /// 画面设置（曝光 / 色调映射 / 泛光）进指纹 —— 它们是后处理 pass 的输入。
    /// 漏掉的话"在 Project Settings 里把曝光从 1.0 拖到 2.0"视口纹丝不动
    /// （静态帧被指纹门控拦下，只有下次相机/场景变化时才补画一次）。
    #[test]
    fn signature_tracks_render_settings() {
        use xgame_core::ToneMapMode;

        let entities = [entity("Crate", "builtin://cube", Vec3::ZERO)];
        let lights = [Light::default()];
        let ambient = [0.06, 0.06, 0.08];

        let hashed = |render: RenderConfig| {
            SceneRenderer::scene_signature(
                (800, 600),
                &Mat4::IDENTITY,
                &Vec3::new(5.0, 5.0, 5.0),
                &SceneLighting::new(ambient, &lights),
                &entities,
                0,
                render,
            )
        };

        let base = hashed(RenderConfig::default());
        assert_ne!(
            base,
            hashed(RenderConfig {
                exposure: 2.0,
                ..Default::default()
            }),
            "改曝光没进指纹（调曝光视口不动）"
        );
        assert_ne!(
            base,
            hashed(RenderConfig {
                tone_map: ToneMapMode::Off,
                ..Default::default()
            }),
            "切换色调映射没进指纹"
        );
        assert_ne!(
            base,
            hashed(RenderConfig {
                fxaa: xgame_core::FxaaConfig { enabled: true },
                ..Default::default()
            }),
            "切换 FXAA 没进指纹（勾了它视口不动）"
        );
        // Bloom 三件套也全是后处理输入：漏掉任何一项 = 调它视口纹丝不动。
        for render in [
            RenderConfig {
                bloom: xgame_core::BloomConfig {
                    intensity: 0.6,
                    ..Default::default()
                },
                ..Default::default()
            },
            RenderConfig {
                bloom: xgame_core::BloomConfig {
                    threshold: 2.0,
                    ..Default::default()
                },
                ..Default::default()
            },
            RenderConfig {
                bloom: xgame_core::BloomConfig {
                    knee: 1.0,
                    ..Default::default()
                },
                ..Default::default()
            },
        ] {
            assert_ne!(base, hashed(render), "Bloom 参数没进指纹：{render:?}");
        }
        // 同一份设置必须给出同一个指纹（哈希稳定，不会每帧白重画）。
        assert_eq!(base, hashed(RenderConfig::default()));
    }

    /// 状态机的**参数值**进了指纹 —— 脚本每帧推参数、Inspector DragValue 拖动
    /// 都靠它触发重绘，否则转移换了状态画面却不刷新。
    #[test]
    fn signature_tracks_machine_parameters_and_config() {
        use std::collections::BTreeMap;
        use xgame_core::{AnimCondition, AnimMachine, AnimState, AnimTransition};

        let lights = [Light::default()];
        let ambient = [0.06, 0.06, 0.08];

        let machine = || AnimMachine {
            entry: "Idle".into(),
            parameters: BTreeMap::from([("speed".to_string(), 0.0)]),
            states: vec![AnimState {
                name: "Idle".into(),
                clip: "Survey".into(),
                speed: 1.0,
                looping: true,
            }],
            transitions: vec![AnimTransition {
                from: "Idle".into(),
                to: "Walk".into(),
                when: AnimCondition::AtLeast {
                    param: "speed".into(),
                    value: 0.5,
                },
            }],
        };

        let mut fox = entity("Fox", "Resources/Fox.gltf", Vec3::ZERO);
        fox.animator = Some(xgame_core::Animator::new("ignored"));
        fox.animator.as_mut().unwrap().machine = Some(machine());
        let base = signature(std::slice::from_ref(&fox), &lights, ambient);

        // 只改参数值 —— 结构一个字节没动，指纹也必须变。
        fox.animator.as_mut().unwrap().machine.as_mut().unwrap()
            .parameters
            .insert("speed".to_string(), 0.6);
        let bumped = signature(std::slice::from_ref(&fox), &lights, ambient);
        assert_ne!(base, bumped, "参数值没进指纹（脚本推参数就不重绘）");

        // 改转移阈值也要变。
        fox.animator.as_mut().unwrap().machine.as_mut().unwrap().transitions[0]
            .when = AnimCondition::AtLeast {
            param: "speed".into(),
            value: 0.9,
        };
        let retuned = signature(std::slice::from_ref(&fox), &lights, ambient);
        assert_ne!(bumped, retuned, "转移阈值没进指纹");

        // 状态表（clip/speed/looping）也要变。
        fox.animator.as_mut().unwrap().machine.as_mut().unwrap().states[0]
            .clip = "Walk".into();
        let restated = signature(std::slice::from_ref(&fox), &lights, ambient);
        assert_ne!(retuned, restated, "状态表没进指纹");

        // 去掉机器（退回单剪辑）同样要变。
        fox.animator.as_mut().unwrap().machine = None;
        let stripped = signature(std::slice::from_ref(&fox), &lights, ambient);
        assert_ne!(restated, stripped, "移除状态机没进指纹");

        // 同样的机器再装回去 → 指纹必须回到 base（哈希稳定，与路径无关）。
        fox.animator.as_mut().unwrap().machine = Some(machine());
        assert_eq!(
            base,
            signature(std::slice::from_ref(&fox), &lights, ambient),
            "相同机器配置必须给相同指纹"
        );
    }

    /// 叠加层的**权重**（脚本每帧推）与配置都进指纹 —— 否则增量效果改了画面不动。
    #[test]
    fn signature_tracks_additive_config() {
        use xgame_core::AnimAdditive;

        let lights = [Light::default()];
        let ambient = [0.06, 0.06, 0.08];

        let mut fox = entity("Fox", "Resources/Fox.gltf", Vec3::ZERO);
        fox.animator = Some(xgame_core::Animator::new("Walk"));
        let plain = signature(std::slice::from_ref(&fox), &lights, ambient);

        fox.animator.as_mut().unwrap().additive = Some(AnimAdditive {
            clip: "Run".into(),
            ..Default::default()
        });
        let layered = signature(std::slice::from_ref(&fox), &lights, ambient);
        assert_ne!(plain, layered, "挂上叠加层没进指纹");

        // 权重是脚本每帧推的量 —— 只改权重也必须变。
        fox.animator.as_mut().unwrap().additive.as_mut().unwrap().weight = 0.3;
        let lighter = signature(std::slice::from_ref(&fox), &lights, ambient);
        assert_ne!(layered, lighter, "权重没进指纹（脚本推权重不重绘）");

        // 换叠加剪辑也要变。
        fox.animator.as_mut().unwrap().additive.as_mut().unwrap().clip = "Survey".into();
        let retargeted = signature(std::slice::from_ref(&fox), &lights, ambient);
        assert_ne!(lighter, retargeted, "叠加剪辑没进指纹");

        // 摘掉叠加层回到纯基础。
        fox.animator.as_mut().unwrap().additive = None;
        assert_eq!(
            plain,
            signature(std::slice::from_ref(&fox), &lights, ambient),
            "摘掉叠加层应还原指纹"
        );
    }

    /// 层栈的每层配置（名字 / 模式 / 剪辑 / 权重 / 遮罩）都进指纹 ——
    /// 加层、脚本按名推权重、改遮罩切分都要触发重绘。
    #[test]
    fn signature_tracks_layer_config() {
        use xgame_core::{AnimLayer, AnimLayerMode};

        let lights = [Light::default()];
        let ambient = [0.06, 0.06, 0.08];

        let mut fox = entity("Fox", "Resources/Fox.gltf", Vec3::ZERO);
        fox.animator = Some(xgame_core::Animator::new("Walk"));
        let plain = signature(std::slice::from_ref(&fox), &lights, ambient);

        fox.animator.as_mut().unwrap().layers = vec![AnimLayer {
            name: "upper".into(),
            mode: AnimLayerMode::Override,
            clip: "Survey".into(),
            weight: 0.6,
            mask: vec!["Spine1".into()],
            ..Default::default()
        }];
        let layered = signature(std::slice::from_ref(&fox), &lights, ambient);
        assert_ne!(plain, layered, "加层没进指纹");

        // 权重（脚本按名推的量）变了要变。
        fox.animator.as_mut().unwrap().layers[0].weight = 0.2;
        let lighter = signature(std::slice::from_ref(&fox), &lights, ambient);
        assert_ne!(layered, lighter, "层权重没进指纹");

        // 遮罩切分变了要变：["Spine1"] 与 ["Spine","1"] 的字节流相同，
        // 靠分隔字节才区分得开 —— 这条测试钉住分隔。
        fox.animator.as_mut().unwrap().layers[0].mask =
            vec!["Spine".to_string(), "1".to_string()];
        let masked = signature(std::slice::from_ref(&fox), &lights, ambient);
        assert_ne!(lighter, masked, "遮罩（含切分）没进指纹");

        // 模式变了要变。
        fox.animator.as_mut().unwrap().layers[0].mode = AnimLayerMode::Additive;
        let remoded = signature(std::slice::from_ref(&fox), &lights, ambient);
        assert_ne!(masked, remoded, "求值模式没进指纹");

        // 清空栈回到纯基础。
        fox.animator.as_mut().unwrap().layers.clear();
        assert_eq!(
            plain,
            signature(std::slice::from_ref(&fox), &lights, ambient),
            "清空层栈应还原指纹"
        );
    }

    /// 没有模型的实体根本不会被画出来，所以它的变化**不该**让指纹变 ——
    /// 否则往场景里塞一堆空实体会把每一帧都拖成"必须重画"。
    #[test]
    fn signature_ignores_entities_without_a_model() {
        let lights = [Light::default()];
        let ambient = [0.06, 0.06, 0.08];
        let drawn = [entity("Box", "builtin://cube", Vec3::ZERO)];

        let mut with_empty = drawn.to_vec();
        with_empty.push(entity("Empty", "", Vec3::ZERO));
        assert_eq!(
            signature(&drawn, &lights, ambient),
            signature(&with_empty, &lights, ambient)
        );

        // 连它的名字/位置一起改，仍然不该有影响。
        let mut renamed = drawn.to_vec();
        let mut empty = Entity::new("Renamed");
        empty.transform.position = Vec3::new(9.0, 9.0, 9.0);
        renamed.push(empty);
        assert_eq!(
            signature(&drawn, &lights, ambient),
            signature(&renamed, &lights, ambient)
        );
    }

    /// 关掉一盏灯（强度 0）与删掉它是同一件事 —— 管线会把它过滤掉。
    #[test]
    fn signature_treats_inactive_lights_as_absent() {
        let e = [entity("Box", "builtin://cube", Vec3::ZERO)];
        let ambient = [0.06, 0.06, 0.08];

        let off = Light {
            intensity: 0.0,
            ..Default::default()
        };

        assert_eq!(
            signature(&e, &[], ambient),
            signature(&e, &[off], ambient),
            "强度为 0 的灯不该参与指纹"
        );
    }

    /// 阴影开关 / 分辨率进指纹：它们决定那张深度图的内容与尺寸，
    /// 漏掉的话在 Inspector 里勾掉 `Casts Shadow` 会看到画面纹丝不动。
    #[test]
    fn signature_notices_shadow_settings() {
        let e = [entity("Box", "builtin://cube", Vec3::ZERO)];
        let ambient = [0.06, 0.06, 0.08];

        let mut shadow_on = Light::directional("sun", [0.5, -1.0, 0.5], [1.0; 3], 1.0);
        shadow_on.casts_shadow = true;
        let mut shadow_off = shadow_on.clone();
        shadow_off.casts_shadow = false;

        assert_ne!(
            signature(&e, &[shadow_on.clone()], ambient),
            signature(&e, &[shadow_off], ambient),
            "阴影开关没进指纹"
        );

        let mut hi_res = shadow_on.clone();
        hi_res.shadow_resolution = 4096;
        assert_ne!(
            signature(&e, &[shadow_on], ambient),
            signature(&e, &[hi_res], ambient),
            "阴影分辨率没进指纹"
        );
    }

    /// 层级：**父引用**必须进指纹。
    ///
    /// 这是"指纹收局部变换"这个取舍的关键补丁：把子节点挂到另一个父节点下时，
    /// 子节点自己的局部变换一个字节都没变，但它的世界矩阵（= 着色器真正读到的
    /// 东西）变了。指纹里不带父引用的话，画面上就是"改完层级画面不动"的脏帧。
    #[test]
    fn signature_notices_reparenting() {
        let mut a = entity("A", "builtin://cube", Vec3::new(0.0, 5.0, 0.0));
        let b = entity("B", "builtin://cube", Vec3::new(1.0, 0.0, 0.0));
        a.parent = Some(EntityId::from_raw(42));

        let lights = [Light::default()];
        let ambient = [0.06, 0.06, 0.08];

        // 同样的实体、同样的局部变换，只有父引用不同。
        let without_parent = vec![a.clone(), b.clone()];
        let mut with_parent = without_parent.clone();
        with_parent[1].parent = Some(EntityId::from_raw(42));

        assert_ne!(
            signature(&without_parent, &lights, ambient),
            signature(&with_parent, &lights, ambient),
            "父引用没进指纹 —— 重新挂载父节点后画面不会更新"
        );
    }

    /// 父节点移动会让子节点的**世界矩阵**变化，即使子节点自身的局部变换一个
    /// 字节都没动。指纹按 (局部变换 + 父引用) 哈希，父节点自己动了就必须变 ——
    /// 否则画面上就是拖着父物体、子物体纹丝不动的脏帧。
    #[test]
    fn signature_tracks_parent_motion_with_unchanged_child_local() {
        let mut parent = entity("Parent", "builtin://cube", Vec3::ZERO);
        let child = entity("Child", "builtin://cube", Vec3::ZERO);
        parent.parent = None;
        let mut entities = vec![parent, child];
        // 子节点挂在父节点下（父引用用 0 号实体的 id）。
        entities[1].parent = Some(entities[0].id);

        let lights = [Light::default()];
        let ambient = [0.06, 0.06, 0.08];

        let before = signature(&entities, &lights, ambient);
        // 只动父节点。
        entities[0].transform.position.x += 2.0;
        assert_ne!(
            before,
            signature(&entities, &lights, ambient),
            "父动子跟，指纹必须变化"
        );
    }
}
