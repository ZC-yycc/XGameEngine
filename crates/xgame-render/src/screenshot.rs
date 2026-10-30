//! 离屏截图：不开窗口渲染一帧场景、读回像素、落盘 PNG。
//!
//! # 为什么（两块用途，一个实现）
//!
//! 1. **Agent-Native「程序化采集」**：`xgame-editor --headless --screenshot`。
//!    智能体改完场景 / 材质没有鼠标 —— 截图就是它的眼睛，从"改完看
//!    退出码"进化到"改完看画面"。
//! 2. **渲染改动的自动化验证**：测试直接读回像素做断言。光照换 BRDF、
//!    阴影、材质这类改动，"像素级红线测试"是唯一说得清的验证手段 ——
//!    本模块的测试里有实例（金属面与绝缘面在同一盏灯下谁亮）。
//!
//! # 与编辑器视口的关系（刻意不共用的一层）
//!
//! **渲染管线共用**（[`ScenePipeline`]：同一套着色器、同一套绘制顺序，
//! 截图就是玩家看到的东西）；**宿主层不共用**（编辑器的 `SceneRenderer`
//! 带着视口节流、动画墙钟、指纹门控这些"面板"逻辑，截图只要确定性的
//! 一帧）。这是"单一运行时"纪律的延伸：可以少一层壳，不能多一套画法。
//!
//! # 确定性
//!
//! 同一台机器、同一份场景与动画时钟，渲染结果**逐字节一致**（两次运行
//! 的 PNG 二进制相等，测试钉死）。跨机器不承诺 —— 不同 GPU 的光栅化
//! 细节本来就没有逐位约定。
//!
//! # GPU 依赖与测试纪律
//!
//! 截图必须要有图形设备（与无头运行的其余部分不同）。拿不到适配器时
//! [`ScreenshotRenderer::new`] 返回可读错误；测试统一走"拿不到就跳过"，
//! 让无显卡的 CI 仍然全绿，而本机（有显卡）老老实实跑像素断言。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use xgame_core::{LightKind, RenderConfig, Scene};
use xgame_math::glam::{Mat4, Vec3};

use crate::animation::AnimationDriver;
use crate::gltf_loader::{Bounds, GltfMesh};
use crate::postfx::{HdrTargets, PostFx, HDR_FORMAT};
use crate::scene_pipeline::{
    scene_bounds, supported_sample_count, FrameStats, SceneLighting, ScenePipeline,
};

/// 截图默认垂直视场角（45°）。比编辑器视口（60°）略窄一点 —— 截图
/// 主要用来"看清单个场景"，透视变形小些更接近正投影的审视感。
pub const DEFAULT_FOV_Y: f32 = std::f32::consts::FRAC_PI_4;

/// 空场景（没有任何可绘制实体）自动取景时的兜底半径。
/// 给个不为零的值，保证相机矩阵不退化。
const EMPTY_SCENE_FALLBACK_RADIUS: f32 = 5.0;

/// 自动取景相机的固定后备视轴（场景里没有可用方向光时使用）。
///
/// 取"右上前方"四分之三视角：水平面 45° 给立体感，俯角约 24° 让地面
/// 与立面都露出来 —— 与大多数引擎资源预览的默认机位同款。
pub fn default_shot_direction() -> Vec3 {
    Vec3::new(1.0, 0.62, 1.25).normalize()
}

/// 由场景的主方向光推一个**顺光**的默认视轴。
///
/// 相机放在主光方位再偏 35°、仰角约 26°：拍出来以受光面为主，还能看到
/// 明暗交界 —— 材质（尤其是金属 / 粗糙度）只有在受光面上才看得出来。
/// 把相机扔在光源对侧（很多"固定默认机位"的实际效果）会让所有物体只剩
/// 环境光的死灰，是自动截图最该避免的事。
///
/// 光的数据来自场景本身 ⇒ 结果仍然是确定性的。没有可用的方向光时退回
/// [`default_shot_direction`]。
pub fn default_shot_direction_for(scene: &Scene) -> Vec3 {
    let light = scene
        .lights
        .iter()
        .find(|l| l.kind == LightKind::Directional && l.is_active());
    let Some(light) = light else {
        return default_shot_direction();
    };
    // 光传播方向取反 = 表面指向光源的方向；取其 XZ 方位角再偏 35°。
    let [dx, dy, dz] = light.normalized_direction();
    let to_light = Vec3::new(-dx, -dy, -dz);
    let azimuth = to_light.x.atan2(to_light.z) + 0.61; // +35°
    let elevation = 0.45_f32; // ≈ 26°
    Vec3::new(
        azimuth.sin() * elevation.cos(),
        elevation.sin(),
        azimuth.cos() * elevation.cos(),
    )
}

/// 一台截图相机的姿态。`eye`/`target` 是**世界空间**坐标。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotCamera {
    pub eye: Vec3,
    pub target: Vec3,
    /// 垂直视场角（弧度）。
    pub fov_y: f32,
    /// 上方向。接近垂直俯视 / 仰视时必须换轴，否则 `look_at` 在
    /// forward ∥ up 时算出 NaN —— 与阴影视锥拟合同款防退化处理。
    pub up: Vec3,
}

impl ShotCamera {
    /// 由视点 / 目标 / 视场角构造，`up` 自动选择：视线接近垂直
    /// （|dir.y| > 0.99）时用 Z 轴，否则 Y 轴。
    pub fn looking_at(eye: Vec3, target: Vec3, fov_y: f32) -> Self {
        let view_dir = (target - eye).try_normalize().unwrap_or(-Vec3::Z);
        let up = if view_dir.y.abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        Self {
            eye,
            target,
            fov_y,
            up,
        }
    }
}

/// 由场景包围盒拟合一台"看得全"的相机。
///
/// `bounds` 为 `None`（空场景）时退化成"原点为中心、半径
/// [`EMPTY_SCENE_FALLBACK_RADIUS`] 的球"——仍然给出一台合法相机，
/// 而不是报错：截一张空场景的图（背景色）本身就是合法需求。
///
/// 距离取 `半径 / sin(min(半垂直 FOV, 半水平 FOV))`，再留 6% 余量：
/// 保证包围球**在两条轴上**都完整入画（宽高比 < 1 的竖图以水平视场
/// 为准，宽图以垂直视场为准）。
pub fn frame_bounds(
    bounds: Option<&Bounds>,
    aspect: f32,
    fov_y: f32,
    direction: Vec3,
) -> ShotCamera {
    let (center, radius) = match bounds {
        Some(b) if !b.is_empty() => (b.center(), b.radius().max(0.5)),
        _ => (Vec3::ZERO, EMPTY_SCENE_FALLBACK_RADIUS),
    };
    let fov_y = fov_y.clamp(5.0_f32.to_radians(), 120.0_f32.to_radians());
    let aspect = aspect.max(1e-3);
    let half_v = fov_y * 0.5;
    // 水平半视场角：tan(h) = tan(v) · aspect。宽图时它比 v 大，取小的那条。
    let half_h = (half_v.tan() * aspect).atan();
    let half_min = half_v.min(half_h);
    let dist = radius / half_min.sin().max(1e-3) * 1.06;
    let dir = direction
        .try_normalize()
        .unwrap_or_else(default_shot_direction);
    ShotCamera::looking_at(center + dir * dist, center, fov_y)
}

/// 由相机与场景半径推近 / 远裁剪面。
///
/// 相机在包围球外（正常取景）时：near 在"到球面"之前留 1.5 倍半径的
/// 富余（球是松包围，真实几何总在球内），far 罩住整球再留一倍余量。
/// 相机被放进球内（手动给的机位）时退化为"尽量小但为正"的面 ——
/// 别处不炸比精确重要。
pub fn clip_planes(camera: &ShotCamera, scene_radius: f32) -> (f32, f32) {
    let dist = (camera.eye - camera.target).length().max(1e-3);
    let radius = scene_radius.max(1e-3);
    let near = (dist - radius * 1.5).max(dist * 0.02).max(0.02);
    let far = dist + radius * 4.0 + 20.0;
    (near, far)
}

/// 离屏渲染器：一台设备 + 一套场景管线 + 一组颜色 / 深度附件。
///
/// 生命周期由调用方决定：一次性截图（CLI）就建一次用一次；将来若要
/// "每帧截图"再谈复用。
pub struct ScreenshotRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: ScenePipeline,
    size: (u32, u32),
    /// 最终显示纹理（sRGB 编码、可拷回 CPU 的那张）。
    ///
    /// 场景 pass **不**直接写它 —— 写它的是后处理 pass（色调映射之后）。
    /// 直接写它的话高光在写入时就被硬件裁掉了，色调映射根本看不到真实亮度。
    _ldr: wgpu::Texture,
    ldr_view: wgpu::TextureView,
    /// 场景 pass 的 HDR 附件（颜色 / MSAA / 深度）。
    hdr: HdrTargets,
    /// 全屏后处理 pass（曝光 + 色调映射）。
    postfx: PostFx,
    /// TAA 的历史缓冲与 resolve（`render.taa.enabled` 关着时全程不用，
    /// 历史纹理也不会被创建）。采样数在构造时交给它（深度纹理类型按它选）。
    taa: crate::taa::TaaResolver,
    /// SSAO 乘算（`render.ssao.enabled` 关着时整段跳过，也不建 AO 纹理）。
    ssao: crate::ssao::SsaoPass,
    /// 屏幕空间 UI（后处理之后画到同一张 LDR 上；无头截图也要有 UI）。
    ui: crate::ui_renderer::UiRenderer,
    /// 模型标识 → 已上传的网格（与编辑器 / 游戏同一条加载入口）。
    model_cache: HashMap<String, GltfMesh>,
    /// 磁盘模型的解析基准（`builtin://` 用不到）。
    project_root: PathBuf,
    /// 质量档位的解析结果（LOD 缩放在渲染时乘进 `render.lod_bias`）。
    /// 无项目 / 没配 `quality` 节时 = "今天的行为"。
    quality: xgame_core::QualitySettings,
}

impl ScreenshotRenderer {
    /// 建 GPU 设备与离屏附件。场景 pass 渲进 `Rgba16Float`（HDR），后处理
    /// pass 做色调映射写进 `Rgba8UnormSrgb`（可读回的那张）；MSAA 采样数
    /// 按 **HDR 格式**的能力选（与编辑器视口同一规则 —— 采样数是格式相关的，
    /// 拿显示格式问出来的答案在 HDR 附件上不一定成立）。
    pub fn new(project_root: &Path, width: u32, height: u32) -> Result<Self> {
        Self::build(project_root, width, height, None)
    }

    /// 测试专用：强制 MSAA 采样数。
    ///
    /// FXAA 的价值在**低采样档位**最明显（4× MSAA 已经把几何边缘磨平了）。
    /// 要证明 FXAA 真的在干活，测试得把 MSAA 关到 1×，否则断言只能看到
    /// 很小的像素差，容易变成"阈值调到刚好能过"的假测试。
    #[cfg(test)]
    fn new_with_sample_count(
        project_root: &Path,
        width: u32,
        height: u32,
        sample_count: u32,
    ) -> Result<Self> {
        Self::build(project_root, width, height, Some(sample_count))
    }

    fn build(
        project_root: &Path,
        width: u32,
        height: u32,
        forced_sample_count: Option<u32>,
    ) -> Result<Self> {
        let width = width.max(1);
        let height = height.max(1);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::empty(),
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| {
            anyhow!("找不到可用的 GPU 适配器 —— 截图需要图形设备（CI 无显卡时别传 --screenshot）")
        })?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("screenshot device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            },
            None,
        ))
        .context("创建 GPU 设备失败")?;

        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        // 质量档位：有项目就按它的 `quality` 节取档（测试可强制采样数覆盖）。
        // 无项目（临时目录 / 裸资源）＝ "没配 quality 节"的行为。
        let adapter_msaa = supported_sample_count(&adapter, HDR_FORMAT);
        // 打开一次项目：既取质量档位，又绑给管线（材质资产要用）。
        let project = xgame_core::ProjectHandle::open(project_root).ok();
        let quality = project
            .as_ref()
            .map(|project| project.manifest().quality.resolve(adapter_msaa))
            .unwrap_or_else(|| xgame_core::QualitySettings::reference(adapter_msaa));
        let sample_count = forced_sample_count.unwrap_or(quality.msaa).max(1);
        log::info!(
            "截图渲染：{}（MSAA x{sample_count}）{width}x{height}",
            adapter.get_info().name
        );

        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let ldr = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot ldr color"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            // RENDER_ATTACHMENT：后处理 pass 的输出；COPY_SRC：读回。
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let ldr_view = ldr.create_view(&wgpu::TextureViewDescriptor::default());

        let hdr = HdrTargets::new(&device, "screenshot", width, height, sample_count);
        let mut pipeline = ScenePipeline::new(&device, HDR_FORMAT, sample_count);
        pipeline.set_default_shadow_resolution(quality.shadow_resolution);
        // 有 `project.ron` 才绑定项目：截图也走共享材质资产这条正门。
        // 临时目录 / 单测里的裸资源没有项目清单，绑定失败就退回"只有模型材质"，
        // 不影响任何既有测试。
        if let Some(project) = project {
            pipeline.set_project(Some(project));
        }
        let postfx = PostFx::new(&device, format);
        let taa = crate::taa::TaaResolver::new(&device, sample_count);
        let ssao = crate::ssao::SsaoPass::new(&device, sample_count);
        let ui = crate::ui_renderer::UiRenderer::new(&device, format);

        Ok(Self {
            device,
            queue,
            pipeline,
            size: (width, height),
            _ldr: ldr,
            ldr_view,
            hdr,
            postfx,
            taa,
            ssao,
            ui,
            model_cache: HashMap::new(),
            project_root: project_root.to_path_buf(),
            quality,
        })
    }

    /// 覆盖质量档位（测试用：无头截图要能复现"低配长什么样"）。
    pub fn set_quality(&mut self, quality: xgame_core::QualitySettings) {
        self.quality = quality;
        self.pipeline
            .set_default_shadow_resolution(quality.shadow_resolution);
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// 上一次 `render_scene` 的 UI 绘制读数（批数 / 图元数）。
    pub fn ui_stats(&self) -> crate::ui_renderer::UiStats {
        self.ui.stats()
    }

    /// 上一次 [`render_scene`](Self::render_scene) 的绘制统计（口径见
    /// [`ScenePipeline::frame_stats`]）。
    ///
    /// 实例化是否真的合批了只有这个读数看得见：像素测试能守"画得对"，
    /// 守不了"是不是一批画完的"。
    pub fn frame_stats(&self) -> FrameStats {
        self.pipeline.frame_stats()
    }

    /// 开关视锥剔除（默认开）—— 只给 A/B 测试与基准用。
    ///
    /// 生产路径不该关它：剔除是保守的，开与关的像素必须逐位一致
    /// （`frustum_culling_matches_unculled_pixels` 钉死）。
    pub fn set_frustum_culling(&mut self, enabled: bool) {
        self.pipeline.set_frustum_culling(enabled);
    }

    /// 开关阴影**逐级**剔除（默认开）—— 只给 A/B 测试与基准用。
    ///
    /// 关掉时每级都画全部投影物（引入前的行为）。开与关的像素必须逐位一致
    /// （`cascaded_shadow_culling_matches_unculled_pixels` 钉死）。
    pub fn set_cascaded_shadow_culling(&mut self, enabled: bool) {
        self.pipeline.set_cascaded_shadow_culling(enabled);
    }

    pub fn aspect(&self) -> f32 {
        self.size.0 as f32 / self.size.1.max(1) as f32
    }

    /// 把指定模型直接塞进缓存（键即 `Entity::model` 的值）。
    ///
    /// 测试用：内置图元的材质是写死的常量，像素红线测试需要"同一几何、
    /// 两种材质"的受控输入。给成 public 也是为将来"材质资源覆盖"留的
    /// 正门 —— 但当前只有测试在用。
    pub fn inject_model(&mut self, key: impl Into<String>, mesh: GltfMesh) {
        self.model_cache.insert(key.into(), mesh);
    }

    /// 自动取景相机：框住当前 `model_cache` 能解析出的全部几何。
    ///
    /// 与 [`render_scene`](Self::render_scene) 的懒加载保持一致 ——
    /// 调用前先 `render_scene` 一次（或自行确保模型已加载），否则包围盒
    /// 会偏小。实际上 CLI 走 `render_scene(None)` 这条自动取景路径，
    /// 不需要单独调用本函数；它是给"先问机位、后截图"的调用方留的。
    pub fn default_camera(&self, scene: &Scene) -> ShotCamera {
        let world_matrices = scene.world_matrices();
        let bounds = scene_bounds(&scene.entities, &world_matrices, &self.model_cache);
        frame_bounds(
            bounds.as_ref(),
            self.aspect(),
            DEFAULT_FOV_Y,
            default_shot_direction_for(scene),
        )
    }

    /// 渲染一帧到离屏颜色纹理。
    ///
    /// `camera` 为 `None` 时自动取景（框住整个场景）；返回**实际使用**的
    /// 相机 —— 调用方要把它写进报告（"这张图是从哪拍的"必须可复现）。
    ///
    /// `anim` 是动画播放时钟：静态截图传空驱动即可；想让骨骼姿态与无头
    /// 运行的模拟时长一致，就按同一串 dt tick 出来。
    ///
    /// `settings` 是画面设置（曝光 / 色调映射）。**与独立游戏同源** ——
    /// 智能体截的图就是玩家会看到的画面，不存在"截图特调"。
    pub fn render_scene(
        &mut self,
        scene: &Scene,
        anim: &AnimationDriver,
        camera: Option<ShotCamera>,
        lighting: SceneLighting<'_>,
        settings: RenderConfig,
    ) -> Result<ShotCamera> {
        self.render_scene_with_particles(scene, anim, camera, lighting, settings, &[])
    }

    /// 带粒子的截图。
    ///
    /// 单开一个方法而不是给 `render_scene` 加参数：**绝大多数调用点（含全部
    /// 既有测试）没有粒子**，多一个 `&[]` 只是噪音；而"截图里有没有粒子"
    /// 本身是个明确的语义差别（无头运行必须有，静态取景测试通常没有）。
    #[allow(clippy::too_many_arguments)]
    pub fn render_scene_with_particles(
        &mut self,
        scene: &Scene,
        anim: &AnimationDriver,
        camera: Option<ShotCamera>,
        lighting: SceneLighting<'_>,
        settings: RenderConfig,
        particles: &[xgame_particle::ParticleQuad],
    ) -> Result<ShotCamera> {
        // 模型懒加载：与编辑器 SceneRenderer 同一条入口，失败只警告
        // （画不出来的实体留空，整张图照常出）。
        for entity in &scene.entities {
            // 逐级加载：截图也要能看到"这个距离上该是哪一级"。
            for model in entity.model_paths() {
                if self.model_cache.contains_key(model) {
                    continue;
                }
                match GltfMesh::load(&self.device, &self.queue, &self.project_root, model) {
                    Ok(m) => {
                        self.model_cache.insert(model.to_string(), m);
                    }
                    Err(e) => log::warn!("截图：加载模型 '{model}' 失败: {:#}", e),
                }
            }
        }

        let world_matrices = scene.world_matrices();
        let bounds = scene_bounds(&scene.entities, &world_matrices, &self.model_cache);
        let camera = camera.unwrap_or_else(|| {
            frame_bounds(
                bounds.as_ref(),
                self.aspect(),
                DEFAULT_FOV_Y,
                default_shot_direction_for(scene),
            )
        });
        let radius = bounds
            .map(|b| b.radius())
            .unwrap_or(EMPTY_SCENE_FALLBACK_RADIUS);
        let (near, far) = clip_planes(&camera, radius);

        let view = Mat4::look_at_rh(camera.eye, camera.target, camera.up);
        let proj = Mat4::perspective_rh(camera.fov_y, self.aspect(), near, far);
        let view_proj = proj * view;

        // TAA：本帧的亚像素抖动（关着时一次都不推进，投影逐位不变）。
        // 抖动序列按帧号取模 —— 同机同帧数两次运行完全一致，`--screenshot`
        // 的"逐字节确定"契约在 TAA 开着时也成立。
        let settings = settings.sanitized();
        let taa_on = settings.taa.enabled;
        let jitter = if taa_on {
            self.taa.begin_frame(self.size)
        } else {
            self.taa.reset();
            [0.0, 0.0]
        };
        let jitter_ndc = crate::taa::jitter_to_ndc(jitter, self.size);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("screenshot encoder"),
            });
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
            anim,
            &view_proj,
            jitter_ndc,
            &camera.eye,
            lighting,
            settings.sky,
            // LOD 最终偏置 = 作者的内容旋钮 × 档位缩放（见 `xgame_core::quality`）。
            settings.lod_bias * self.quality.lod_scale,
            particles,
        );
        // TAA resolve（开着时）—— 必须在后处理**之前**：曝光 / 色调映射要作用在
        // 收敛后的画面上，反过来（先 tonemap 再混历史）会把曲线套两遍。
        let (source_view, source_gen) = if taa_on {
            self.taa.resolve(
                &self.device,
                &self.queue,
                &mut encoder,
                &self.hdr.color_view,
                &self.hdr.depth_view,
                self.size,
                &view_proj,
                &camera.eye,
                settings.taa,
            )
        } else {
            (&self.hdr.color_view, 0u64)
        };
        // SSAO 乘算（开着时）—— TAA 之后、后处理之前：AO 要在曝光 / 色调
        // 映射之前乘进场景色。关着时整段跳过（不建纹理、不记 pass）。
        let (source_view, source_gen) = if settings.ssao.enabled {
            self.ssao.apply(
                &self.device,
                &self.queue,
                &mut encoder,
                source_view,
                source_gen,
                &self.hdr.depth_view,
                self.size,
                &view_proj,
                &camera.eye,
                settings.ssao,
            )
        } else {
            (source_view, source_gen)
        };
        // HDR 场景图 → 曝光 +（Bloom）→ 色调映射 → LDR 显示目标。截图器建一次
        // 用一次、目标从不重建 ⇒ 目标代数恒为 0（绑定组缓存只填一次）；
        // Bloom 链按尺寸惰性建，`source_size` 就是渲染尺寸。
        self.postfx.render(
            &self.device,
            &self.queue,
            &mut encoder,
            source_view,
            source_gen,
            self.size,
            &self.ldr_view,
            settings,
        );
        // UI：截图与玩家看到的必须是同一张画面 —— 只画 3D 的截图会漏掉 HUD。
        self.ui.prepare(
            &self.device,
            &self.queue,
            scene,
            self.size,
            &self.project_root,
        );
        self.ui.render(
            &self.device,
            &self.queue,
            &mut encoder,
            &self.ldr_view,
            self.size,
        );
        self.queue.submit(std::iter::once(encoder.finish()));
        Ok(camera)
    }

    /// 场景 pass 的采样数（1 = 未开 MSAA）。
    ///
    /// 给测试用：`taa_runs_on_the_multisampled_path` 要先确认本机适配器真的
    /// 开了 MSAA，否则它测的是另一条分支却"通过"了 —— 那比不测更糟。
    pub fn sample_count(&self) -> u32 {
        self.pipeline.sample_count()
    }

    /// 把颜色纹理读回 CPU（RGBA8、行紧排）。
    ///
    /// 拷贝的行距按 WebGPU 的 256 字节对齐要求补齐，读回后再把补丁裁掉。
    pub fn read_rgba(&self) -> Result<Vec<u8>> {
        let (width, height) = self.size;
        let unpadded_row = width as usize * 4;
        let padded_row = unpadded_row.div_ceil(256) * 256;

        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot readback"),
            size: (padded_row * height as usize) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("screenshot readback encoder"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &self._ldr,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row as u32),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|_| anyhow!("读回通道断开（渲染设备丢失？）"))?
            .map_err(|e| anyhow!("映射读回缓冲失败: {e}"))?;

        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity(unpadded_row * height as usize);
        for row in data.chunks_exact(padded_row) {
            out.extend_from_slice(&row[..unpadded_row]);
        }
        drop(data);
        buffer.unmap();
        Ok(out)
    }

    /// 读回像素并落盘 PNG（目录不存在会自动创建）。返回图像尺寸。
    pub fn save_png(&self, path: &Path) -> Result<(u32, u32)> {
        let (width, height) = self.size;
        let rgba = self.read_rgba()?;
        save_rgba_png(path, &rgba, width, height)?;
        Ok((width, height))
    }
}

/// 把一段 RGBA8 像素写成 PNG（目录不存在会自动创建）。
///
/// 与 [`ScreenshotRenderer::save_png`] 共用同一段落盘逻辑 —— 编辑器的
/// 「界面截图」（`--ui-shot`）抓的是 egui 的上屏像素，不是场景渲染器，
/// 但落盘格式必须一致，否则两套工具读图要写两个分支。
pub fn save_rgba_png(path: &Path, rgba: &[u8], width: u32, height: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建截图目录失败 ({})", parent.display()))?;
        }
    }
    image::save_buffer_with_format(
        path,
        rgba,
        width,
        height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .with_context(|| format!("写 PNG 失败 ({})", path.display()))?;
    Ok(())
}

/// 读一张 PNG 回 RGBA8（测试 / 验证工具用：免得到处再引一遍 image 依赖）。
pub fn load_png_rgba(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let img = image::open(path)
        .with_context(|| format!("打开 PNG 失败 ({})", path.display()))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    Ok((w, h, img.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use xgame_core::{
        BloomConfig, Entity, FxaaConfig, Light, MaterialAsset, MaterialOverride, SkyConfig,
        TaaConfig, ToneMapMode,
    };
    use xgame_math::glam::Quat;

    use crate::sky::SkyUniform;

    /// 纯函数测试用的盒（边长 2，中心在 (1, 2, 3) 附近，测试取景的平移。
    fn test_bounds() -> Bounds {
        Bounds {
            min: Vec3::new(0.0, 0.0, 0.0),
            max: Vec3::new(2.0, 4.0, 6.0),
        }
    }

    /// 把包围盒八个角点投到裁剪空间：每个角都必须在 NDC 立方内
    /// （|x|,|y| ≤ 1，z ∈ [0,1]）——这才叫"看得全"。
    fn corners_inside_at(aspect: f32, dir: Vec3) {
        let bounds = test_bounds();
        let cam = frame_bounds(Some(&bounds), aspect, DEFAULT_FOV_Y, dir);
        let (near, far) = clip_planes(&cam, bounds.radius());
        let view = Mat4::look_at_rh(cam.eye, cam.target, cam.up);
        let proj = Mat4::perspective_rh(cam.fov_y, aspect, near, far);
        let view_proj = proj * view;
        for corner in bounds.corners() {
            let clip = view_proj * corner.extend(1.0);
            let ndc = clip.truncate() / clip.w;
            assert!(
                ndc.x.abs() <= 1.0 + 1e-3 && ndc.y.abs() <= 1.0 + 1e-3,
                "aspect={aspect} 方向 {dir:?} 下角点 {corner:?} 出画: ndc={ndc:?}"
            );
            assert!(
                (0.0..=1.0).contains(&ndc.z),
                "角点 {corner:?} 被深度裁剪: z={}",
                ndc.z
            );
        }
    }

    #[test]
    fn frame_bounds_fits_every_corner_across_aspects() {
        for aspect in [0.5, 4.0 / 3.0, 1.0, 2.5] {
            corners_inside_at(aspect, default_shot_direction());
        }
    }

    #[test]
    fn frame_bounds_fits_every_corner_for_off_axis_directions() {
        for dir in [
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(-1.0, 0.1, -1.0),
            Vec3::new(1.0, -0.2, 0.3),
        ] {
            corners_inside_at(16.0 / 9.0, dir);
        }
    }

    #[test]
    fn frame_bounds_keeps_direction_and_target() {
        let bounds = test_bounds();
        let dir = Vec3::new(1.0, 1.0, -1.0).normalize();
        let cam = frame_bounds(Some(&bounds), 16.0 / 9.0, DEFAULT_FOV_Y, dir);
        assert!((cam.eye - cam.target).normalize().abs_diff_eq(dir, 1e-5));
        assert!(cam.target.abs_diff_eq(bounds.center(), 1e-5));
    }

    #[test]
    fn frame_bounds_empty_scene_still_produces_a_sane_camera() {
        let cam = frame_bounds(None, 16.0 / 9.0, DEFAULT_FOV_Y, default_shot_direction());
        assert_eq!(cam.target, Vec3::ZERO);
        assert!(cam.eye.is_finite());
        assert!((cam.eye - cam.target).length() > EMPTY_SCENE_FALLBACK_RADIUS);
    }

    /// 默认取景必须"顺光"：相机视轴与"表面指向光源"的方向同侧。
    /// 背光机位拍出来物体只剩环境光死灰，材质全看不出来。
    #[test]
    fn default_direction_faces_the_sun() {
        let mut scene = Scene::new("sun");
        scene.lights = vec![Light::directional("sun", [0.45, -1.0, 0.35], [1.0; 3], 3.2)];
        let dir = default_shot_direction_for(&scene);
        let to_light = Vec3::new(-0.45, 1.0, -0.35).normalize();
        let alignment = dir.dot(to_light);
        assert!(
            alignment > 0.5,
            "默认视轴应当明显偏向光源侧（点积 = {alignment:.2}）"
        );

        // 换一个光源方位：结果跟着转，仍然顺光。
        scene.lights[0].direction = [-1.0, -0.6, 0.0];
        let dir2 = default_shot_direction_for(&scene);
        let to_light2 = Vec3::new(1.0, 0.6, 0.0).normalize();
        assert!(dir2.dot(to_light2) > 0.5);
        assert!(
            dir2.distance(dir) > 0.5,
            "光源方位换了，默认机位不应该原地不动"
        );
    }

    /// 没有可用方向光（只有点光 / 强度为 0）时退回固定方向，结果仍然确定。
    #[test]
    fn default_direction_falls_back_without_directional_light() {
        let mut scene = Scene::new("no-sun");
        scene.lights.clear();
        assert_eq!(default_shot_direction_for(&scene), default_shot_direction());

        scene.lights = vec![Light::point("lamp", [0.0, 3.0, 0.0], [1.0; 3], 2.0, 10.0)];
        assert_eq!(default_shot_direction_for(&scene), default_shot_direction());

        scene.lights = vec![Light::directional("dead", [0.0, -1.0, 0.0], [1.0; 3], 0.0)];
        assert_eq!(default_shot_direction_for(&scene), default_shot_direction());
    }

    #[test]
    fn clip_planes_bracket_the_scene_sphere() {
        let cam = frame_bounds(
            Some(&test_bounds()),
            16.0 / 9.0,
            DEFAULT_FOV_Y,
            default_shot_direction(),
        );
        let radius = test_bounds().radius();
        let (near, far) = clip_planes(&cam, radius);
        let dist = (cam.eye - cam.target).length();
        assert!(
            near > 0.0 && near < dist + radius,
            "near={near} dist={dist}"
        );
        assert!(far > dist + radius, "far={far} 必须罩住整个包围球");
    }

    #[test]
    fn clip_planes_stay_positive_inside_the_sphere() {
        let cam = ShotCamera::looking_at(Vec3::new(0.1, 0.1, 0.1), Vec3::ZERO, DEFAULT_FOV_Y);
        let (near, far) = clip_planes(&cam, 50.0);
        assert!(near > 0.0 && near.is_finite(), "near={near}");
        assert!(far > near);
    }

    /// 接近垂直的视线必须换 up 轴 —— 否则 `look_at` 的 forward ∥ up 会
    /// 算出 NaN，整张图静默变黑（NaN 通过投影矩阵传染到所有顶点）。
    #[test]
    fn top_down_camera_switches_up_axis_and_stays_finite() {
        let bounds = test_bounds();
        let straight_down = Vec3::new(0.0, 1.0, 0.0);
        let cam = frame_bounds(Some(&bounds), 16.0 / 9.0, DEFAULT_FOV_Y, straight_down);
        assert_eq!(cam.up, Vec3::Z, "垂直俯视时 up 必须换 Z 轴");
        let view = Mat4::look_at_rh(cam.eye, cam.target, cam.up);
        assert!(
            view.to_cols_array().iter().all(|v| v.is_finite()),
            "退化视角不能算出 NaN 矩阵: {view:?}"
        );
        // 取景照样完整（角点全在画面内）。
        corners_inside_at(16.0 / 9.0, straight_down);
    }

    // -----------------------------------------------------------------
    // GPU 测试（拿不到适配器就跳过，见模块头"GPU 依赖与测试纪律"）
    // -----------------------------------------------------------------

    /// GPU 测试并发上限。
    ///
    /// 本机（16 逻辑核，默认 16 个测试线程）在同时创建十几个 wgpu 设备时会
    /// **挂死**：进程不前进、没有报错、也没有 GPU 占用。8 线程跑同一组测试
    /// 3 秒即绿，所以问题在驱动侧的并发设备创建，而不是某条测试本身。
    ///
    /// 这不是产品代码的问题，但会让 `cargo test` 变成"永远跑不完"，测试
    /// 安全网就此失效。用一个计数信号量把**同时存活的截图渲染器**限制住；
    /// 4 个足够让 CPU 侧并行，又离驱动的临界点有安全距离。
    const MAX_CONCURRENT_GPU_TESTS: usize = 4;

    struct GpuGate {
        active: std::sync::Mutex<usize>,
        cv: std::sync::Condvar,
    }

    static GPU_GATE: std::sync::OnceLock<GpuGate> = std::sync::OnceLock::new();

    struct GpuTestPermit {
        gate: &'static GpuGate,
    }

    impl Drop for GpuTestPermit {
        fn drop(&mut self) {
            let mut active = self.gate.active.lock().unwrap_or_else(|e| e.into_inner());
            *active -= 1;
            self.gate.cv.notify_one();
        }
    }

    fn acquire_gpu_test_permit() -> GpuTestPermit {
        let gate = GPU_GATE.get_or_init(|| GpuGate {
            active: std::sync::Mutex::new(0),
            cv: std::sync::Condvar::new(),
        });
        let mut active = gate.active.lock().unwrap_or_else(|e| e.into_inner());
        while *active >= MAX_CONCURRENT_GPU_TESTS {
            active = gate.cv.wait(active).unwrap_or_else(|e| e.into_inner());
        }
        *active += 1;
        GpuTestPermit { gate }
    }

    /// 带并发许可的截图渲染器：`Deref` 到 [`ScreenshotRenderer`]，测试代码
    /// 照常 `shot.render_scene(...)` / `shot.device`，许可在测试结束时归还。
    struct GpuFixture {
        renderer: ScreenshotRenderer,
        _permit: GpuTestPermit,
    }

    impl std::ops::Deref for GpuFixture {
        type Target = ScreenshotRenderer;
        fn deref(&self) -> &Self::Target {
            &self.renderer
        }
    }

    impl std::ops::DerefMut for GpuFixture {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.renderer
        }
    }

    /// 建一个截图渲染器；无 GPU 环境跳过（返回 None）。
    fn gpu_or_skip(name: &str) -> Option<GpuFixture> {
        let permit = acquire_gpu_test_permit();
        match ScreenshotRenderer::new(&std::env::temp_dir(), 320, 240) {
            Ok(renderer) => Some(GpuFixture {
                renderer,
                _permit: permit,
            }),
            Err(e) => {
                eprintln!("跳过 {name}：{e}");
                None
            }
        }
    }

    /// FXAA 的价值在高对比度、**低采样**的斜边上最明显：用 1× MSAA 渲染
    /// 一个绕 Z 轴旋转的薄盒，斜边必然是一格一格的硬台阶。
    fn gpu_or_skip_msaa1(name: &str) -> Option<GpuFixture> {
        let permit = acquire_gpu_test_permit();
        match ScreenshotRenderer::new_with_sample_count(&std::env::temp_dir(), 320, 240, 1) {
            Ok(renderer) => Some(GpuFixture {
                renderer,
                _permit: permit,
            }),
            Err(e) => {
                eprintln!("跳过 {name}：{e}");
                None
            }
        }
    }

    /// 临时项目目录（测试结束删掉整棵子树）。
    struct TestProjectDir(PathBuf);

    impl Drop for TestProjectDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 建一个带材质资产的临时项目，并让截图渲染器绑定它。
    ///
    /// 顺序是刻意的：先写贴图，再写 `.mat` —— `write_material` 会把
    /// `albedo` 编码成 GUID，贴图必须先被资产库扫到，否则落盘的是字面量路径。
    fn material_gpu_fixture(
        name: &str,
        textures: &[(&str, [u8; 4])],
        materials: &[(&str, MaterialAsset)],
    ) -> Option<(GpuFixture, TestProjectDir)> {
        let permit = acquire_gpu_test_permit();
        let parent = std::env::temp_dir().join(format!(
            "xgame_material_shot_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&parent).expect("建临时项目目录失败");
        let guard = TestProjectDir(parent.clone());

        let project = match xgame_core::ProjectHandle::create(&parent, "MaterialProject") {
            Ok(project) => project,
            Err(e) => {
                eprintln!("跳过 {name}：创建临时项目失败: {e}");
                return None;
            }
        };

        for (relative, rgba) in textures {
            let path = project.path(relative);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).expect("建贴图目录失败");
            }
            let mut pixels = Vec::with_capacity(8 * 8 * 4);
            for _ in 0..8 * 8 {
                pixels.extend_from_slice(rgba);
            }
            image::save_buffer_with_format(
                &path,
                &pixels,
                8,
                8,
                image::ColorType::Rgba8,
                image::ImageFormat::Png,
            )
            .expect("写测试贴图失败");
        }

        for (relative, material) in materials {
            project
                .write_material(relative, material)
                .expect("写测试材质失败");
        }

        match ScreenshotRenderer::new(project.root(), 320, 240) {
            Ok(renderer) => Some((
                GpuFixture {
                    renderer,
                    _permit: permit,
                },
                guard,
            )),
            Err(e) => {
                eprintln!("跳过 {name}：{e}");
                None
            }
        }
    }

    /// 斜射光下的一颗白模立方体 —— 材质像素测试的受控画面。
    fn material_probe_scene() -> Scene {
        let mut scene = Scene::new("material");
        scene.ambient = [0.0, 0.0, 0.0];
        scene.lights = vec![Light::directional("sun", [0.6, -1.0, -0.75], [1.0; 3], 1.0)];
        scene.lights[0].casts_shadow = false;
        let mut e = Entity::new("Cube");
        e.model = "builtin://cube".to_owned();
        scene.add_entity(e);
        scene
    }

    fn fxaa_probe_scene() -> Scene {
        let mut scene = Scene::new("fxaa-edge");
        scene.ambient = [0.01, 0.01, 0.01];
        scene.lights = vec![Light::directional("key", [0.25, -0.6, -1.0], [1.0; 3], 2.5)];
        scene.lights[0].casts_shadow = false;
        let mut blade = Entity::new("Blade");
        blade.model = "builtin://cube".to_owned();
        blade.transform.rotation = Quat::from_rotation_z(0.35);
        blade.transform.scale = Vec3::new(4.0, 2.6, 1.4);
        scene.add_entity(blade);
        scene
    }

    fn luma_of(p: [u8; 3]) -> f32 {
        (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0
    }

    /// 相邻像素的亮度跳变超过 `threshold` 的次数（横 + 纵两向）。
    ///
    /// 硬台阶在 FXAA 之后会多出一两个中间值，于是"大幅跳变"变少；
    /// 平坦区域即使有轻微噪声也不会被计入。
    fn hard_luma_steps(rgba: &[u8], width: u32, height: u32, threshold: f32) -> usize {
        let luma = |x: u32, y: u32| luma_of(pixel_at(rgba, width, x, y));
        let mut count = 0;
        for y in 0..height {
            for x in 0..width - 1 {
                if (luma(x + 1, y) - luma(x, y)).abs() > threshold {
                    count += 1;
                }
            }
        }
        for y in 0..height - 1 {
            for x in 0..width {
                if (luma(x, y + 1) - luma(x, y)).abs() > threshold {
                    count += 1;
                }
            }
        }
        count
    }

    // -----------------------------------------------------------------
    // TAA（时域抗锯齿）：GPU 侧像素测试组
    // -----------------------------------------------------------------

    /// 在同一个渲染器上连跑 `frames` 帧（TAA 需要历史累积），返回最后一帧。
    fn render_frames(
        shot: &mut ScreenshotRenderer,
        scene: &Scene,
        anim: &AnimationDriver,
        camera: ShotCamera,
        settings: RenderConfig,
        frames: usize,
    ) -> Vec<u8> {
        let mut last = Vec::new();
        for _ in 0..frames.max(1) {
            shot.render_scene(
                scene,
                anim,
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                settings,
            )
            .expect("渲染失败");
            last = shot.read_rgba().expect("读回失败");
        }
        last
    }

    fn taa_on() -> RenderConfig {
        RenderConfig {
            taa: TaaConfig {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// TAA 真的在干活：1× MSAA 的斜边硬台阶必须明显变少、中间调变多。
    ///
    /// 用 12 帧（一个抖动周期 8 帧 + 收敛余量）取"相机静止、画面收敛后"的样子
    /// —— 这正是 TAA 最有价值的场景（静态画面的边缘质量）。判据沿用 FXAA
    /// 那套 `hard_luma_steps` / `midtone_pixels`，两种抗锯齿因此可以直接对表。
    #[test]
    fn taa_smooths_edges_after_a_few_frames() {
        let Some(mut shot) = gpu_or_skip_msaa1("taa_smooths_edges_after_a_few_frames") else {
            return;
        };
        let scene = fxaa_probe_scene();
        let anim = AnimationDriver::new();
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 8.0), Vec3::ZERO, DEFAULT_FOV_Y);

        let off = render_frames(&mut shot, &scene, &anim, camera, RenderConfig::default(), 1);
        let on = render_frames(&mut shot, &scene, &anim, camera, taa_on(), 12);

        assert_ne!(off, on, "TAA 开关没接进渲染链：两张图逐字节相同");

        // 背景取左上角、前景取画面中心（薄盒正好压在视轴上）。
        let bg = luma_of(pixel_at(&off, 320, 4, 4));
        let fg = luma_of(pixel_at(&off, 320, 160, 120));
        assert!(
            (fg - bg).abs() > 0.15,
            "测试前提：前景 / 背景必须有足够对比度（bg={bg:.3} fg={fg:.3}）"
        );

        let steps_off = hard_luma_steps(&off, 320, 240, 0.25);
        let steps_on = hard_luma_steps(&on, 320, 240, 0.25);
        assert!(
            steps_off > 200,
            "测试前提：1× MSAA 的斜边必须有大量硬台阶（{steps_off}）"
        );
        assert!(
            steps_on < steps_off,
            "TAA 应当把硬台阶磨成中间调：{steps_off} → {steps_on}"
        );

        let mid_off = midtone_pixels(&off, 320, 240, bg, fg);
        let mid_on = midtone_pixels(&on, 320, 240, bg, fg);
        assert!(
            mid_on > mid_off,
            "TAA 必须增加斜边上的中间调像素：{mid_off} → {mid_on}"
        );
        // 读数留在输出里（`--nocapture` 可见）：这是"TAA 到底有多有效"的量化证据，
        // 也是将来调 `history_weight` / 抖动周期时的基准。
        eprintln!(
            "TAA 12 帧：硬台阶 {steps_off} → {steps_on}（-{:.0}%），中间调 {mid_off} → {mid_on}",
            (1.0 - steps_on as f32 / steps_off as f32) * 100.0
        );
    }

    /// TAA 开着时截图仍然**逐字节确定**：两个全新渲染器各跑 5 帧，PNG 相同。
    ///
    /// 抖动序列是纯函数（Halton 按帧号取模）、历史缓冲从零开始，所以"第 5 帧"
    /// 在两次运行里必须是同一张图 —— 这是 `--screenshot` 确定性的契约在 TAA
    /// 下的延续（序列一旦引入随机数或墙钟，这条立刻失效）。
    #[test]
    fn taa_keeps_two_runs_byte_identical() {
        let Some(mut first) = gpu_or_skip("taa_keeps_two_runs_byte_identical") else {
            return;
        };
        let scene = fxaa_probe_scene();
        let anim = AnimationDriver::new();
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 8.0), Vec3::ZERO, DEFAULT_FOV_Y);

        let dir = std::env::temp_dir().join(format!(
            "xgame_taa_det_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let path_a = dir.join("a.png");
        let path_b = dir.join("b.png");

        render_frames(&mut first, &scene, &anim, camera, taa_on(), 5);
        first.save_png(&path_a).expect("落盘失败");

        // 全新渲染器 = 新设备 / 新历史缓冲，等价于"另一个进程跑一遍"。
        let mut second =
            ScreenshotRenderer::new(&std::env::temp_dir(), 320, 240).expect("第二次创建渲染器失败");
        render_frames(&mut second, &scene, &anim, camera, taa_on(), 5);
        second.save_png(&path_b).expect("落盘失败");

        let bytes_a = std::fs::read(&path_a).unwrap();
        let bytes_b = std::fs::read(&path_b).unwrap();
        assert_eq!(bytes_a, bytes_b, "TAA 开着时两次运行的 PNG 必须逐字节一致");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **首帧**必须是一张正常画面：平均亮度与不抖动的同一帧几乎相同。
    ///
    /// 这条守的是被真实抓到过一次的 bug：`resolve` 首帧若把"刚读的那张历史"
    /// 当输出返回，后处理读到的是一张**从未写过**的纹理 —— 整屏全黑。
    /// 12 帧的收敛测试发现不了它（落后的那张上一帧已经写过，只是慢一帧），
    /// 只有"只看第一帧"能抓住。
    #[test]
    fn taa_first_frame_is_not_black() {
        let Some(mut shot) = gpu_or_skip_msaa1("taa_first_frame_is_not_black") else {
            return;
        };
        let scene = fxaa_probe_scene();
        let anim = AnimationDriver::new();
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 8.0), Vec3::ZERO, DEFAULT_FOV_Y);

        let mean = |rgba: &[u8]| -> f32 {
            let (pixels, _) = rgba.as_chunks::<4>();
            let sum: f32 = pixels.iter().map(|p| luma_of([p[0], p[1], p[2]])).sum();
            sum / pixels.len() as f32
        };

        let off = render_frames(&mut shot, &scene, &anim, camera, RenderConfig::default(), 1);
        let first = render_frames(&mut shot, &scene, &anim, camera, taa_on(), 1);

        let (m_off, m_on) = (mean(&off), mean(&first));
        assert!(
            m_on > 0.05,
            "TAA 首帧整屏发黑（平均亮度 {m_on:.3}）—— 检查 resolve 输出的视图"
        );
        assert!(
            (m_on - m_off).abs() < 0.05,
            "TAA 首帧应当≈不抖动的同一帧（{m_off:.3} vs {m_on:.3}）：首帧不该直接混历史"
        );
    }

    /// 默认采样数（本机常见 4×）下 TAA 也必须能跑。
    ///
    /// 这条守的是**多采样深度**那条分支：深度附件是多采样时，WGSL 里的类型必须
    /// 换成 `texture_depth_multisampled_2d`，否则绑定组校验直接失败 —— 而那正是
    /// 编辑器视口与独立游戏（都按适配器档位开 MSAA）走的路径。1× 的用例覆盖不到。
    #[test]
    fn taa_runs_on_the_multisampled_path() {
        let Some(mut shot) = gpu_or_skip("taa_runs_on_the_multisampled_path") else {
            return;
        };
        if shot.renderer.sample_count() <= 1 {
            // 适配器只有 1× 档位：这条分支在本机不存在，别假装测了。
            eprintln!("跳过 taa_runs_on_the_multisampled_path：适配器不支持 MSAA");
            return;
        }
        let scene = fxaa_probe_scene();
        let anim = AnimationDriver::new();
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 8.0), Vec3::ZERO, DEFAULT_FOV_Y);

        let off = render_frames(&mut shot, &scene, &anim, camera, RenderConfig::default(), 1);
        // 两帧就够：第一帧直通（历史为空），第二帧才真正读历史 —— 乒乓与
        // 重投影两条路径都走到。跑完不 panic / 不出设备错误 = 这条分支通了。
        let on = render_frames(&mut shot, &scene, &anim, camera, taa_on(), 2);
        assert_ne!(off, on, "TAA 在 MSAA 路径下没有生效");
    }

    // -----------------------------------------------------------------
    // SSAO（屏幕空间环境光遮蔽）：GPU 侧像素测试组
    // -----------------------------------------------------------------

    fn ssao_on() -> RenderConfig {
        RenderConfig {
            ssao: xgame_core::SsaoConfig {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// 两个靠在一起的立方体（内角）：遮蔽最明显的地方。
    fn ssao_probe_scene() -> Scene {
        let mut scene = Scene::new("ssao-corner");
        scene.ambient = [0.35, 0.35, 0.35];
        scene.lights = vec![Light::directional("key", [0.3, -0.8, -1.0], [1.0; 3], 1.2)];
        scene.lights[0].casts_shadow = false;
        let mut floor = Entity::new("Floor");
        floor.model = "builtin://cube".to_owned();
        floor.transform.position = Vec3::new(0.0, -0.6, 0.0);
        floor.transform.scale = Vec3::new(4.0, 1.2, 4.0);
        scene.add_entity(floor);
        let mut wall = Entity::new("Wall");
        wall.model = "builtin://cube".to_owned();
        wall.transform.position = Vec3::new(0.0, 0.6, -1.2);
        wall.transform.scale = Vec3::new(4.0, 1.2, 0.6);
        scene.add_entity(wall);
        scene
    }

    /// 平均亮度。
    fn mean_luma(rgba: &[u8]) -> f32 {
        let (pixels, _) = rgba.as_chunks::<4>();
        let sum: f32 = pixels.iter().map(|p| luma_of([p[0], p[1], p[2]])).sum();
        sum / pixels.len() as f32
    }

    /// 逐像素最大变暗量（off − on，只计正变暗）。
    fn max_darken(off: &[u8], on: &[u8]) -> f32 {
        let (pa, _) = off.as_chunks::<4>();
        let (pb, _) = on.as_chunks::<4>();
        pa.iter()
            .zip(pb.iter())
            .map(|(a, b)| luma_of([a[0], a[1], a[2]]) - luma_of([b[0], b[1], b[2]]))
            .fold(0.0f32, f32::max)
    }

    /// SSAO 真的在干活：开着的画面必须整体更暗（AO 只会乘 ≤1 的系数），
    /// 且接触角上必须有看得见的压暗（不是只差几个量化级的"名义生效"）。
    #[test]
    fn ssao_darkens_contact_regions() {
        let Some(mut shot) = gpu_or_skip("ssao_darkens_contact_regions") else {
            return;
        };
        let scene = ssao_probe_scene();
        let anim = AnimationDriver::new();
        let camera = ShotCamera::looking_at(Vec3::new(2.6, 2.2, 3.6), Vec3::ZERO, DEFAULT_FOV_Y);

        let off = render_frames(&mut shot, &scene, &anim, camera, RenderConfig::default(), 1);
        let on = render_frames(&mut shot, &scene, &anim, camera, ssao_on(), 1);
        assert_ne!(off, on, "SSAO 开关没接进渲染链：两张图逐字节相同");

        let (m_off, m_on) = (mean_luma(&off), mean_luma(&on));
        assert!(
            m_on < m_off,
            "SSAO 只会压暗：均值不该变亮（off={m_off:.4} on={m_on:.4}）"
        );
        let darken = max_darken(&off, &on);
        assert!(
            darken > 0.03,
            "接触角的压暗量必须看得出来（最大单像素变暗 {darken:.4}）"
        );
        eprintln!("SSAO：均值 {m_off:.4} → {m_on:.4}，最大单像素变暗 {darken:.4}");
    }

    /// SSAO 开着时截图仍然**逐字节确定**：两个全新渲染器各渲一帧，PNG 相同。
    ///
    /// 噪声是像素坐标的整数哈希（无随机相位、无时间项），kernel 是固定种子
    /// LCG —— 序列一旦引入随机数或墙钟，这条立刻失效。
    #[test]
    fn ssao_keeps_two_runs_byte_identical() {
        let Some(mut first) = gpu_or_skip("ssao_keeps_two_runs_byte_identical") else {
            return;
        };
        let scene = ssao_probe_scene();
        let anim = AnimationDriver::new();
        let camera = ShotCamera::looking_at(Vec3::new(2.6, 2.2, 3.6), Vec3::ZERO, DEFAULT_FOV_Y);

        let dir = std::env::temp_dir().join(format!(
            "xgame_ssao_det_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let path_a = dir.join("a.png");
        let path_b = dir.join("b.png");

        render_frames(&mut first, &scene, &anim, camera, ssao_on(), 1);
        first.save_png(&path_a).expect("落盘失败");

        // 全新渲染器 = 新设备 / 新 AO 纹理，等价于"另一个进程跑一遍"。
        let mut second =
            ScreenshotRenderer::new(&std::env::temp_dir(), 320, 240).expect("第二次创建渲染器失败");
        render_frames(&mut second, &scene, &anim, camera, ssao_on(), 1);
        second.save_png(&path_b).expect("落盘失败");

        let bytes_a = std::fs::read(&path_a).unwrap();
        let bytes_b = std::fs::read(&path_b).unwrap();
        assert_eq!(bytes_a, bytes_b, "SSAO 开着时两次运行的 PNG 必须逐字节一致");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `strength = 0` 时画面必须与关着**逐位一致**（AO 着色器早退成全 1，
    /// 乘算变直通）—— "强度 0 = 无遮蔽效果"的契约。
    #[test]
    fn ssao_strength_zero_matches_disabled() {
        let Some(mut shot) = gpu_or_skip("ssao_strength_zero_matches_disabled") else {
            return;
        };
        let scene = ssao_probe_scene();
        let anim = AnimationDriver::new();
        let camera = ShotCamera::looking_at(Vec3::new(2.6, 2.2, 3.6), Vec3::ZERO, DEFAULT_FOV_Y);

        let off = render_frames(&mut shot, &scene, &anim, camera, RenderConfig::default(), 1);
        let zero = render_frames(
            &mut shot,
            &scene,
            &anim,
            camera,
            RenderConfig {
                ssao: xgame_core::SsaoConfig {
                    enabled: true,
                    strength: 0.0,
                    ..Default::default()
                },
                ..Default::default()
            },
            1,
        );
        assert_eq!(off, zero, "strength=0 必须与关着逐位一致");
    }

    /// 介于前景 / 背景之间的中间调像素数：抗锯齿的"灰边"就是这些像素。
    fn midtone_pixels(rgba: &[u8], width: u32, height: u32, bg: f32, fg: f32) -> usize {
        let lo = f32::min(bg, fg) + 0.06;
        let hi = f32::max(bg, fg) - 0.06;
        let mut count = 0;
        for y in 0..height {
            for x in 0..width {
                let l = luma_of(pixel_at(rgba, width, x, y));
                if l > lo && l < hi {
                    count += 1;
                }
            }
        }
        count
    }

    /// 单个内置立方体、一盏开着的方向光（无阴影，简化像素预测）。
    fn one_cube_scene() -> Scene {
        let mut scene = Scene::new("shot");
        scene.ambient = [0.0, 0.0, 0.0];
        scene.lights = vec![Light::directional("sun", [0.0, -1.0, 0.0], [1.0; 3], 1.0)];
        scene.lights[0].casts_shadow = false;
        let mut e = Entity::new("Cube");
        e.model = "builtin://cube".to_owned();
        scene.add_entity(e);
        scene
    }

    fn pixel_at(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * width + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2]]
    }

    /// 3x3 均值（抗 MSAA 边缘的单像素抖动）。
    fn pixel_avg(rgba: &[u8], width: u32, x: u32, y: u32) -> [f32; 3] {
        let mut sum = [0.0f32; 3];
        let mut n = 0.0;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let px = (x as i32 + dx).max(0) as u32;
                let py = (y as i32 + dy).max(0) as u32;
                let p = pixel_at(rgba, width, px, py);
                sum[0] += p[0] as f32;
                sum[1] += p[1] as f32;
                sum[2] += p[2] as f32;
                n += 1.0;
            }
        }
        [sum[0] / n, sum[1] / n, sum[2] / n]
    }

    /// 渲染 → 读回：画面必须真的画出了东西（不是纯背景色）。
    #[test]
    fn render_produces_non_uniform_pixels() {
        let Some(mut shot) = gpu_or_skip("render_produces_non_uniform_pixels") else {
            return;
        };
        let scene = one_cube_scene();
        let anim = AnimationDriver::new();
        shot.render_scene(
            &scene,
            &anim,
            None,
            SceneLighting::new(scene.ambient, &scene.lights),
            RenderConfig::default(),
        )
        .expect("渲染失败");

        let rgba = shot.read_rgba().expect("读回失败");
        assert_eq!(rgba.len(), 320 * 240 * 4);
        let (pixels, _) = rgba.as_chunks::<4>();
        let first = &pixels[0];
        let any_different = pixels.iter().any(|p| p != first);
        assert!(any_different, "整张图一个颜色 = 几何没画出来");
    }

    /// 同一场景渲染两遍（连渲染器都重建）：PNG 二进制逐字节一致。
    /// 这是 `--screenshot` 确定性契约在模块层的钉子。
    ///
    /// 特意开满 Bloom + FXAA：Bloom 链的惰性建（首帧重建纹理与绑定组）、
    /// 9 趟 pass 的记录顺序、FXAA 的 scratch 惰性建与二段合成都必须确定 ——
    /// 关着测只覆盖合成 pass 一条路径。
    #[test]
    fn two_runs_produce_identical_png_bytes() {
        let Some(mut first) = gpu_or_skip("two_runs_produce_identical_png_bytes") else {
            return;
        };
        let scene = one_cube_scene();
        let anim = AnimationDriver::new();

        let dir = std::env::temp_dir().join(format!(
            "xgame_shot_det_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_a = dir.join("a.png");
        let path_b = dir.join("b.png");
        let settings = RenderConfig {
            fxaa: FxaaConfig { enabled: true },
            ..bloom_settings(0.7)
        };

        first
            .render_scene(
                &scene,
                &anim,
                None,
                SceneLighting::new(scene.ambient, &scene.lights),
                settings,
            )
            .expect("第一次渲染失败");
        let (w, h) = first.save_png(&path_a).expect("第一次落盘失败");

        // 重建整个渲染器（新设备、新管线、新附件），模拟"两次进程运行"。
        let mut second =
            ScreenshotRenderer::new(&std::env::temp_dir(), 320, 240).expect("第二次创建渲染器失败");
        second
            .render_scene(
                &scene,
                &anim,
                None,
                SceneLighting::new(scene.ambient, &scene.lights),
                settings,
            )
            .expect("第二次渲染失败");
        second.save_png(&path_b).expect("第二次落盘失败");

        let bytes_a = std::fs::read(&path_a).unwrap();
        let bytes_b = std::fs::read(&path_b).unwrap();
        assert_eq!(bytes_a, bytes_b, "两次截图的 PNG 必须逐字节一致");

        // 顺带验证 PNG 读写闭环：尺寸正确、像素与显存读回一致（无损）。
        let (pw, ph, pixels) = load_png_rgba(&path_a).expect("PNG 应能读回");
        assert_eq!((pw, ph), (w, h));
        assert_eq!(pixels.len(), 320 * 240 * 4);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------
    // 天空盒 + 环境光照（IBL）：GPU 侧像素测试组
    // -----------------------------------------------------------------

    /// 天空测试的共用画面设置：**ToneMap Off + 曝光 1 + 无 Bloom**。
    /// GPU 输出于是就是 `sky_radiance` 钳制后的 sRGB —— 与 CPU 镜像逐值可比，
    /// 不给色调映射曲线留"差一点也算对"的模糊空间。
    fn sky_settings(enabled: bool) -> RenderConfig {
        RenderConfig {
            tone_map: ToneMapMode::Off,
            exposure: 1.0,
            bloom: BloomConfig {
                intensity: 0.0,
                ..Default::default()
            },
            sky: SkyConfig {
                enabled,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// 与 `fs_sky` 逐式对应的"像素中心 → 世界方向"（CPU 对表用）。
    ///
    /// 用的是渲染器自己那条 `inv_view_proj · (ndc, 1, 1) − eye` 路径 —— 对表
    /// 测试最怕两边各用一套相机数学，"差半个像素"会被误判成公式不一致。
    fn sky_dir_for_pixel(
        camera: &ShotCamera,
        aspect: f32,
        radius: f32,
        (w, h): (u32, u32),
        (x, y): (u32, u32),
    ) -> Vec3 {
        let (near, far) = clip_planes(camera, radius);
        let view_proj = Mat4::perspective_rh(camera.fov_y, aspect, near, far)
            * Mat4::look_at_rh(camera.eye, camera.target, camera.up);
        let inv = view_proj.inverse();
        let ndc_x = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
        let ndc_y = 1.0 - (y as f32 + 0.5) / h as f32 * 2.0;
        let far_p = inv * xgame_math::glam::Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
        (far_p.truncate() / far_p.w - camera.eye).normalize()
    }

    /// 天空盒的公式只有一份：GPU 背景必须等于 CPU `sky_radiance` 的镜像。
    ///
    /// 取天顶、地平线、地面与左右两侧五个点。没有方向光（没有太阳盘），
    /// 剩下的是平滑渐变 —— 抗锯齿与盘面边界不会污染读数。
    #[test]
    fn sky_background_matches_cpu_mirror() {
        let Some(mut shot) = gpu_or_skip("sky_background_matches_cpu_mirror") else {
            return;
        };
        let scene = Scene::new("sky");
        let camera = ShotCamera::looking_at(
            Vec3::new(1.5, 2.0, 3.0),
            Vec3::new(0.0, 1.0, 0.0),
            DEFAULT_FOV_Y,
        );
        let settings = sky_settings(true);
        shot.render_scene(
            &scene,
            &AnimationDriver::new(),
            Some(camera),
            SceneLighting::new(scene.ambient, &scene.lights),
            settings,
        )
        .expect("渲染失败");
        let rgba = shot.read_rgba().expect("读回失败");
        let (w, h) = shot.size();
        let uniform = SkyUniform::build(&settings.sky, None);

        for (x, y) in [
            (w / 2, 12),
            (w / 2, h / 2),
            (w / 2, h - 12),
            (12, h / 2),
            (w - 12, h / 2),
        ] {
            let dir = sky_dir_for_pixel(
                &camera,
                shot.aspect(),
                EMPTY_SCENE_FALLBACK_RADIUS,
                (w, h),
                (x, y),
            );
            let cpu = crate::postfx::cpu::to_srgb_u8(crate::postfx::cpu::tone_map(
                crate::sky::cpu::sky_radiance(&uniform, dir),
                settings,
            ));
            let gpu = pixel_at(&rgba, w, x, y);
            for ch in 0..3 {
                let delta = gpu[ch] as i32 - cpu[ch] as i32;
                assert!(
                    delta.abs() <= 2,
                    "像素 ({x},{y}) 通道 {ch}: GPU {} vs CPU {}（方向 {dir:?}）",
                    gpu[ch],
                    cpu[ch]
                );
            }
        }
    }

    /// 关着天空时背景必须还是旧的清屏色 —— "默认关不改变旧项目画面"的钉子。
    ///
    /// 天空盒一旦漏掉开关位（或画出第一帧后就不再清屏），这条会立刻红。
    #[test]
    fn sky_off_keeps_the_old_background_clear_color() {
        let Some(mut shot) = gpu_or_skip("sky_off_keeps_the_old_background_clear_color") else {
            return;
        };
        let scene = Scene::new("old");
        shot.render_scene(
            &scene,
            &AnimationDriver::new(),
            None,
            SceneLighting::new(scene.ambient, &scene.lights),
            sky_settings(false),
        )
        .expect("渲染失败");
        let rgba = shot.read_rgba().expect("读回失败");
        let gpu = pixel_at(&rgba, shot.size().0, 5, 5);
        let expected = crate::postfx::cpu::to_srgb_u8([0.12, 0.12, 0.14]);
        for ch in 0..3 {
            assert!(
                (gpu[ch] as i32 - expected[ch] as i32).abs() <= 1,
                "关着天空时背景必须是旧清屏色: GPU {gpu:?} vs {expected:?}"
            );
        }
    }

    /// **IBL 照亮无光绝缘体**：没有直接光、ambient 也清零时，开天空前立方体
    /// 必须全黑；开天空后必须被球谐环境光照亮。
    ///
    /// 这同时钉住"SH 系数真的写进了 binding 8"和"场景 FS 的 IBL 分支真的
    /// 走得到"—— 只建管线不写数据时，两边都是黑，方向性断言会红。
    #[test]
    fn sky_ibl_lights_an_unlit_insulator() {
        let Some(mut shot) = gpu_or_skip("sky_ibl_lights_an_unlit_insulator") else {
            return;
        };
        let mut scene = one_cube_scene();
        scene.lights.clear();
        scene.ambient = [0.0; 3];
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, DEFAULT_FOV_Y);
        let anim = AnimationDriver::new();
        let luma = |p: [f32; 3]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];

        shot.render_scene(
            &scene,
            &anim,
            Some(camera),
            SceneLighting::new(scene.ambient, &scene.lights),
            sky_settings(false),
        )
        .expect("关天空渲染失败");
        let off = pixel_avg(&shot.read_rgba().expect("读回失败"), 320, 160, 120);

        shot.render_scene(
            &scene,
            &anim,
            Some(camera),
            SceneLighting::new(scene.ambient, &scene.lights),
            sky_settings(true),
        )
        .expect("开天空渲染失败");
        let on = pixel_avg(&shot.read_rgba().expect("读回失败"), 320, 160, 120);

        assert!(
            luma(off) < 2.0,
            "无直接光 + 无环境光时立方体应当是黑的: {off:?}"
        );
        assert!(
            luma(on) > 20.0,
            "天空 IBL 应当照亮无光绝缘体: {on:?}（关天空时 {off:?}）"
        );
        assert!(
            luma(on) > luma(off) + 20.0,
            "开天空必须带来可见的环境照明: on={on:?} off={off:?}"
        );
    }

    /// **立方体面约定 + 预过滤链的端到端钉子**：光滑金属平面上的太阳反光
    /// 必须出现在"几何镜像"算出的屏幕位置。
    ///
    /// 场景里那盏方向光强度是 1e-30（只为给天空提供太阳方向与颜色，直接光
    /// 可忽略），亮点只能来自 `env_cube`。六面基向量任何一处转错 / 镜像，
    /// 太阳就会出现在别处甚至不出现 —— `cube_face_basis` 的历史错误
    /// （+Y 面上下颠倒）正是会在这里被抓住。
    #[test]
    fn metal_reflects_the_sky_sun_at_the_mirrored_screen_position() {
        let Some(mut shot) =
            gpu_or_skip("metal_reflects_the_sky_sun_at_the_mirrored_screen_position")
        else {
            return;
        };

        let builtin = crate::mesh::BuiltinMesh::Plane;
        let mesh = {
            let mut mesh = GltfMesh::from_mesh_data(
                &shot.device,
                &shot.queue,
                &builtin.build(),
                builtin.label(),
            );
            for prim in &mut mesh.primitives {
                prim.material.metallic = 1.0;
                prim.material.roughness = 0.05;
                // 金属的漫反射为零，基色只剩很弱的 F0 底色；把反射对比拉开。
                prim.material.base_color = [0.02, 0.02, 0.02, 1.0];
            }
            mesh
        };
        shot.inject_model("test://mirror", mesh);

        // 指向太阳（方向光的传播方向取反）。
        let sun_dir = Vec3::new(0.35, 0.55, -0.75).normalize();
        let mut scene = Scene::new("mirror");
        scene.ambient = [0.0; 3];
        scene.lights = vec![Light::directional(
            "sun",
            (-sun_dir).to_array(),
            [1.0; 3],
            1e-30,
        )];
        scene.lights[0].casts_shadow = false;
        let mut plane = Entity::new("Mirror");
        plane.model = "test://mirror".to_owned();
        plane.transform.scale = Vec3::new(40.0, 1.0, 40.0);
        scene.add_entity(plane);

        let camera = ShotCamera::looking_at(Vec3::new(0.0, 5.0, 10.0), Vec3::ZERO, DEFAULT_FOV_Y);
        let settings = RenderConfig {
            sky: SkyConfig {
                enabled: true,
                zenith: [0.0; 3],
                horizon: [0.0; 3],
                ground: [0.0; 3],
                sun_size: 8.0,
                sun_intensity: 1.0,
                intensity: 1.0,
            },
            ..sky_settings(true)
        };
        shot.render_scene(
            &scene,
            &AnimationDriver::new(),
            Some(camera),
            SceneLighting::new(scene.ambient, &scene.lights),
            settings,
        )
        .expect("渲染失败");
        let rgba = shot.read_rgba().expect("读回失败");
        let (w, h) = shot.size();

        // 渲染器取用的 near/far 由场景包围盒半径决定 —— 对表必须用同一组。
        let world = scene.world_matrices();
        let bounds = scene_bounds(&scene.entities, &world, &shot.model_cache);
        let radius = bounds
            .map(|b| b.radius())
            .unwrap_or(EMPTY_SCENE_FALLBACK_RADIUS);
        let (near, far) = clip_planes(&camera, radius);
        let view_proj = Mat4::perspective_rh(camera.fov_y, shot.aspect(), near, far)
            * Mat4::look_at_rh(camera.eye, camera.target, camera.up);
        let inv = view_proj.inverse();

        // 平面镜里的太阳 = 把太阳方向按平面法线镜像后，从眼睛看过去的交点。
        let mirrored = Vec3::new(sun_dir.x, -sun_dir.y, sun_dir.z);
        let t = -camera.eye.y / mirrored.y;
        let expected_point = camera.eye + mirrored * t;
        assert!(
            expected_point.x.abs() < 20.0 && expected_point.z.abs() < 20.0,
            "测试机位没把镜像点放在平面上: {expected_point:?}"
        );
        let clip = view_proj * expected_point.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        let expected = (
            (ndc.x * 0.5 + 0.5) * w as f32,
            (1.0 - ndc.y) * 0.5 * h as f32,
        );

        // 只在真的打到平面上的像素里找太阳盘。盘内辐射亮度经 ToneMap Off
        // 后全部钳到 255（整块饱和），所以用**饱和像素的质心**对位置 ——
        // 取"第一个最亮点"会落在盘的上边缘，平白引入一个盘半径的偏差。
        let mut sum_x = 0f64;
        let mut sum_y = 0f64;
        let mut count = 0u32;
        let mut brightest = 0u8;
        for y in 0..h {
            for x in 0..w {
                let ndc_x = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
                let ndc_y = 1.0 - (y as f32 + 0.5) / h as f32 * 2.0;
                let far_p = inv * xgame_math::glam::Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
                let dir = (far_p.truncate() / far_p.w - camera.eye).normalize();
                if dir.y >= -1e-6 {
                    continue;
                }
                let hit = camera.eye + dir * (-camera.eye.y / dir.y);
                if hit.x.abs() > 19.0 || hit.z.abs() > 19.0 {
                    continue;
                }
                let px = pixel_at(&rgba, w, x, y);
                let lum = px[0].max(px[1]).max(px[2]);
                brightest = brightest.max(lum);
                if lum >= 200 {
                    sum_x += x as f64;
                    sum_y += y as f64;
                    count += 1;
                }
            }
        }

        assert!(
            brightest > 200 && count > 0,
            "镜面平面应当反射出天空里的太阳（找不到亮点）"
        );
        let (cx, cy) = (sum_x / count as f64, sum_y / count as f64);
        let dist = ((cx - expected.0 as f64).powi(2) + (cy - expected.1 as f64).powi(2)).sqrt();
        assert!(
            dist < 15.0,
            "太阳反光的质心与几何镜像不符：实际 ({cx:.1}, {cy:.1})，期望 ({:.1}, {:.1})，距离 {dist:.1}px",
            expected.0,
            expected.1
        );
    }

    /// 天空开着时整条链（球谐投影 + 30 趟预过滤 + BRDF LUT + 天空盒 + IBL）
    /// 也必须逐位确定：重建渲染器再跑一遍，PNG 二进制相等。
    #[test]
    fn sky_enabled_two_runs_produce_identical_png_bytes() {
        let Some(mut first) = gpu_or_skip("sky_enabled_two_runs_produce_identical_png_bytes")
        else {
            return;
        };
        let scene = one_cube_scene();
        let anim = AnimationDriver::new();
        let settings = RenderConfig {
            bloom: BloomConfig {
                intensity: 0.5,
                ..Default::default()
            },
            ..sky_settings(true)
        };

        let dir = std::env::temp_dir().join(format!(
            "xgame_sky_det_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_a = dir.join("a.png");
        let path_b = dir.join("b.png");

        first
            .render_scene(
                &scene,
                &anim,
                None,
                SceneLighting::new(scene.ambient, &scene.lights),
                settings,
            )
            .expect("第一次渲染失败");
        first.save_png(&path_a).expect("第一次落盘失败");

        let mut second =
            ScreenshotRenderer::new(&std::env::temp_dir(), 320, 240).expect("第二次创建渲染器失败");
        second
            .render_scene(
                &scene,
                &anim,
                None,
                SceneLighting::new(scene.ambient, &scene.lights),
                settings,
            )
            .expect("第二次渲染失败");
        second.save_png(&path_b).expect("第二次落盘失败");

        let bytes_a = std::fs::read(&path_a).unwrap();
        let bytes_b = std::fs::read(&path_b).unwrap();
        assert_eq!(bytes_a, bytes_b, "天空开着时两次截图的 PNG 必须逐字节一致");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 验证范围确实被片元着色器消费，同时保留近处旧亮度。
    #[test]
    fn regression_r09_point_range_changes_pixels() {
        let Some(mut shot) = gpu_or_skip("regression_r09_point_range_changes_pixels") else {
            return;
        };
        let builtin = crate::mesh::BuiltinMesh::Cube;
        let mut mesh =
            GltfMesh::from_mesh_data(&shot.device, &shot.queue, &builtin.build(), builtin.label());
        for prim in &mut mesh.primitives {
            prim.material.metallic = 0.0;
            prim.material.roughness = 0.9;
        }
        shot.inject_model("test://range", mesh);
        let mut scene = Scene::new("范围回归");
        scene.ambient = [0.0; 3];
        let mut entity = Entity::new("立方体");
        entity.model = "test://range".into();
        scene.add_entity(entity);
        let mut samples = Vec::new();
        for range in [1.0, 10.0, 20.0, 2.5, 2.5 / 0.9, 0.0, -1.0] {
            scene.lights = vec![Light::point("点光", [0.0, 0.0, 3.0], [1.0; 3], 0.3, range)];
            shot.render_scene(
                &scene,
                &AnimationDriver::new(),
                Some(ShotCamera::looking_at(
                    Vec3::new(0.0, 0.0, 4.0),
                    Vec3::ZERO,
                    DEFAULT_FOV_Y,
                )),
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .unwrap();
            samples.push(pixel_avg(&shot.read_rgba().unwrap(), 320, 160, 120));
        }
        assert!(
            samples[1][0] > samples[0][0] + 30.0,
            "范围外应熄灭，范围内应照亮：{samples:?}"
        );
        assert_eq!(samples[1], samples[2], "近处保留旧衰减，不随 range 改亮度");
        assert_eq!(samples[0], samples[3], "边界贡献为零");
        assert!(
            samples[4][0] > samples[0][0] && samples[4][0] < samples[1][0],
            "收尾区应平滑衰减：{samples:?}"
        );
        assert_eq!(samples[0], samples[5]);
        assert_eq!(samples[0], samples[6]);
    }

    /// **PBR 红线测试**：同一盏灯、同一几何，金属面（metallic=1）的中心像素
    /// 必须比绝缘面（metallic=0）**暗** —— 金属没有漫反射，能量全在镜面项，
    /// 高粗糙度下几乎不反光。旧 Blinn-Phong 实现恰好相反（把金属度翻译成
    /// "更大的高光强度"，金属更亮），所以这条测试在换 BRDF 前必然是红的。
    #[test]
    fn metallic_surface_is_darker_than_dielectric_under_same_light() {
        let Some(mut shot) = gpu_or_skip("metallic_surface_is_darker_than_dielectric") else {
            return;
        };

        let builtin = crate::mesh::BuiltinMesh::Cube;
        // 闭包借用 shot 的设备 / 队列；先造好两块网格，闭包退出（借用结束）
        // 再注入缓存 —— 绕过"不可变借用与可变借用并存"。
        let meshes: Vec<(&str, GltfMesh)> = {
            let make = |metallic: f32, roughness: f32| {
                let mut mesh = GltfMesh::from_mesh_data(
                    &shot.device,
                    &shot.queue,
                    &builtin.build(),
                    builtin.label(),
                );
                for prim in &mut mesh.primitives {
                    prim.material.metallic = metallic;
                    prim.material.roughness = roughness;
                }
                mesh
            };
            vec![
                ("test://dielectric", make(0.0, 0.9)),
                ("test://metal", make(1.0, 0.9)),
            ]
        };
        for (key, mesh) in meshes {
            shot.inject_model(key, mesh);
        }

        let mut scene = Scene::new("pbr");
        scene.ambient = [0.0, 0.0, 0.0];
        scene.lights = vec![Light::directional("sun", [0.0, 0.0, -1.0], [1.0; 3], 0.3)];
        scene.lights[0].casts_shadow = false;
        let mut left = Entity::new("Dielectric");
        left.model = "test://dielectric".to_owned();
        left.transform.position = Vec3::new(-0.7, 0.0, 0.0);
        scene.add_entity(left);
        let mut right = Entity::new("Metal");
        right.model = "test://metal".to_owned();
        right.transform.position = Vec3::new(0.7, 0.0, 0.0);
        scene.add_entity(right);

        // 正视两台立方体正面（+z 面朝相机），方向光的传播方向 -z 打亮该面。
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, DEFAULT_FOV_Y);
        shot.render_scene(
            &scene,
            &AnimationDriver::new(),
            Some(camera),
            SceneLighting::new(scene.ambient, &scene.lights),
            RenderConfig::default(),
        )
        .expect("渲染失败");

        let rgba = shot.read_rgba().expect("读回失败");
        // 立方体半边长 0.5、心距 0.7、相机 z=4：投影后两中心在图像中心 ±58px 附近，
        // 取中心行的两侧采样点，各做 3x3 均值。
        let (cx, cy) = (320 / 2, 240 / 2);
        let dielectric = pixel_avg(&rgba, 320, cx - 58, cy);
        let metal = pixel_avg(&rgba, 320, cx + 58, cy);
        let luma = |p: [f32; 3]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
        let (dl, ml) = (luma(dielectric), luma(metal));
        assert!(
            dl > 60.0,
            "绝缘面应当被明显照亮（采样点是不是打到背景上了？）: {dielectric:?}"
        );
        // 定量预期（按 GGX 手算过）：绝缘 ≈ 0.30 线性（漫反射 + 微弱高光），
        // 金属 ≈ 0.12 线性（只剩粗糙镜面项）—— 线性比 ≈ 0.4；经 ACES 曲线
        // 压缩后显示亮度比 ≈ 0.58。阈值放宽到 0.8 留出 GPU 光栅化与 sRGB
        // 编码的余量，但"金属必须明显更暗"这条方向性断言是硬的
        // （旧 Blinn-Phong 下金属 184 > 绝缘 152，方向相反）。
        assert!(
            ml < dl * 0.8,
            "金属面必须比绝缘面暗（无漫反射）: 金属 luma={ml:.1} vs 绝缘 {dl:.1}"
        );
        assert!(ml > 10.0, "金属仍应反射一点光（粗糙镜面项）: {metal:?}");
    }

    /// **逐实体材质覆盖**：给内置白模写 `material.base_color` 之后，渲染出的
    /// 中心像素必须真的带上这个色调 —— 覆盖从场景数据一路到 GPU 的钉子。
    /// 在这条测试之前，内置图元没有任何泛化手段上色（白模就是白模）。
    #[test]
    fn entity_material_override_tints_builtin_mesh() {
        let Some(mut shot) = gpu_or_skip("entity_material_override_tints_builtin_mesh") else {
            return;
        };

        let mut scene = Scene::new("tint");
        scene.ambient = [0.0, 0.0, 0.0];
        // 光斜着来：中心像素落在"漫反射为主、镜面无瓣"的位置。光要是与相机
        // 同轴（0,0,-1），N·H 恰为 1，粗糙度 0.45 的 GGX 副瓣会给采样点叠上
        // 约 0.24 线性白（实测），把乘子的效果糊掉一半。
        scene.lights = vec![Light::directional("sun", [0.6, -1.0, -0.75], [1.0; 3], 1.0)];
        scene.lights[0].casts_shadow = false;
        let mut e = Entity::new("Cube");
        e.model = "builtin://cube".to_owned();
        scene.add_entity(e);

        // 正视 +z 面；方向光的传播方向含 -z 分量，该面受光（N·L ≈ 0.56）。
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, DEFAULT_FOV_Y);
        let (cx, cy) = (320 / 2, 240 / 2);

        // ① 基线（无覆盖）：白模正面受光 → 亮灰。
        let render = |scene: &Scene, shot: &mut ScreenshotRenderer| {
            shot.render_scene(
                scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .expect("渲染失败");
            pixel_avg(&shot.read_rgba().expect("读回失败"), 320, cx, cy)
        };
        let baseline = render(&scene, &mut shot);
        assert!(
            baseline[0] > 150.0,
            "基线白模应当受光明显（采样点打到背景上？）: {baseline:?}"
        );

        // ② 蓝色乘子覆盖：蓝通道必须压过红通道，红通道被乘子显著压低。
        // 定量预期：受光面线性亮度 ≈ 0.54，经 ACES 显示 ≈ 201；红 =
        // 0.2×0.54 ≈ 0.11（显示 ≈ 92），蓝 = 0.9×0.54 ≈ 0.49（显示 ≈ 195）。
        // 阈值留编码与光栅化的余量。
        scene.entities[0].material = Some(MaterialOverride {
            base_color: Some([0.2, 0.4, 0.9, 1.0]),
            ..Default::default()
        });
        let tinted = render(&scene, &mut shot);
        assert!(
            tinted[2] > tinted[0] + 60.0,
            "蓝色乘子必须让蓝通道显著压过红通道: {tinted:?}"
        );
        assert!(
            tinted[0] < baseline[0] * 0.65,
            "红通道应当被乘子显著压低: {tinted:?} vs 基线 {baseline:?}"
        );

        // ③ 再清掉覆盖：像素必须回到基线（覆盖不是一次性烘培）。
        scene.entities[0].material = None;
        let restored = render(&scene, &mut shot);
        assert_eq!(restored, baseline, "清掉覆盖后画面必须与基线逐位一致");
    }

    /// **共享材质资产**：`.mat` 从项目加载后必须真正作用到内置白模上，
    /// 且清掉引用后逐位回到基线（不是一次性烘焙）。
    ///
    /// 这条测试覆盖「项目句柄 → `.mat` 解析 → GPU 绑定组 → 批次键」整条链；
    /// 只做 CPU 侧解析测试的话，"文件对了但画面没变"这类断裂查不出来。
    #[test]
    fn material_asset_tints_builtin_mesh() {
        let material = MaterialAsset {
            base_color: [0.2, 0.4, 0.9, 1.0],
            ..Default::default()
        };
        let Some((mut shot, _project)) = material_gpu_fixture(
            "material_asset_tints_builtin_mesh",
            &[],
            &[("Resources/Materials/Blue.mat", material)],
        ) else {
            return;
        };

        let mut scene = material_probe_scene();
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, DEFAULT_FOV_Y);
        let (cx, cy) = (320 / 2, 240 / 2);
        let render = |scene: &Scene, shot: &mut ScreenshotRenderer| {
            shot.render_scene(
                scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .expect("渲染失败");
            pixel_avg(&shot.read_rgba().expect("读回失败"), 320, cx, cy)
        };

        let baseline = render(&scene, &mut shot);
        assert!(
            baseline[0] > 150.0,
            "基线白模应当受光明显（采样点打到背景上？）: {baseline:?}"
        );

        scene.entities[0].material_asset = Some("Resources/Materials/Blue.mat".to_string());
        let tinted = render(&scene, &mut shot);
        assert!(
            tinted[2] > tinted[0] + 60.0,
            "材质资产的蓝色乘子必须让蓝通道压过红通道: {tinted:?}"
        );
        assert!(
            tinted[0] < baseline[0] * 0.65,
            "红通道应当被材质乘子显著压低: {tinted:?} vs 基线 {baseline:?}"
        );

        scene.entities[0].material_asset = None;
        let restored = render(&scene, &mut shot);
        assert_eq!(restored, baseline, "清掉材质引用后画面必须与基线逐位一致");
    }

    /// **材质 albedo**：`.mat` 里的贴图（磁盘上以 GUID 存）必须替换模型贴图，
    /// 并经过 `texture_srgb` 的默认 sRGB 解码 —— 成品红贴图应当只留红通道。
    #[test]
    fn material_asset_albedo_texture_applies() {
        let material = MaterialAsset {
            albedo: "Resources/Textures/red.png".to_string(),
            ..Default::default()
        };
        let Some((mut shot, _project)) = material_gpu_fixture(
            "material_asset_albedo_texture_applies",
            &[("Resources/Textures/red.png", [255, 0, 0, 255])],
            &[("Resources/Materials/Red.mat", material)],
        ) else {
            return;
        };

        let mut scene = material_probe_scene();
        scene.entities[0].material_asset = Some("Resources/Materials/Red.mat".to_string());
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, DEFAULT_FOV_Y);
        shot.render_scene(
            &scene,
            &AnimationDriver::new(),
            Some(camera),
            SceneLighting::new(scene.ambient, &scene.lights),
            RenderConfig::default(),
        )
        .expect("渲染失败");

        let rgba = shot.read_rgba().expect("读回失败");
        let center = pixel_avg(&rgba, 320, 320 / 2, 240 / 2);
        assert!(
            center[0] > 100.0,
            "红贴图必须让红通道保持明显亮度: {center:?}"
        );
        assert!(
            center[0] > center[1] + 60.0,
            "贴图应按 sRGB 解码后只保留红通道（绿/蓝是镜面项残余）: {center:?}"
        );
    }

    /// **批次键必须含材质**：两颗同一图元、不同 `.mat` 的立方体不能合批；
    /// 改成同一份资产后才允许合并。旧键（图元地址）下这条会读到 1 批，
    /// 表现为后一颗把前一颗的材质盖掉。
    #[test]
    fn material_assets_split_opaque_batches() {
        let red = MaterialAsset {
            base_color: [0.9, 0.1, 0.1, 1.0],
            ..Default::default()
        };
        let blue = MaterialAsset {
            base_color: [0.1, 0.1, 0.9, 1.0],
            ..Default::default()
        };
        let Some((mut shot, _project)) = material_gpu_fixture(
            "material_assets_split_opaque_batches",
            &[],
            &[
                ("Resources/Materials/Red.mat", red),
                ("Resources/Materials/Blue.mat", blue),
            ],
        ) else {
            return;
        };

        let mut scene = material_probe_scene();
        let mut second = Entity::new("Cube2");
        second.model = "builtin://cube".to_owned();
        second.transform.position.x = 1.4;
        scene.entities[0].transform.position.x = -1.4;
        scene.entities[0].material_asset = Some("Resources/Materials/Red.mat".to_string());
        scene.add_entity(second);
        scene.entities[1].material_asset = Some("Resources/Materials/Blue.mat".to_string());

        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, DEFAULT_FOV_Y);
        let render = |scene: &Scene, shot: &mut ScreenshotRenderer| {
            shot.render_scene(
                scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .expect("渲染失败");
            shot.frame_stats()
        };

        let split = render(&scene, &mut shot);
        assert_eq!(
            split.opaque_batches, 2,
            "不同材质资产必须拆成两批（否则一颗的颜色会盖掉另一颗）: {split:?}"
        );

        // 指向同一份资产：批次键相同，必须重新合回一批。
        scene.entities[1].material_asset = Some("Resources/Materials/Red.mat".to_string());
        let merged = render(&scene, &mut shot);
        assert_eq!(
            merged.opaque_batches, 1,
            "同一图元 + 同一材质应合批: {merged:?}"
        );
    }

    // -----------------------------------------------------------------
    // 粒子：CPU 模拟的输出必须真的画进画面，且遵守深度 / 混合语义
    // -----------------------------------------------------------------

    /// 一颗加色粒子放在相机正前方：中心像素必须被点亮，且**颜色就是粒子的
    /// 颜色**（不是被雾 / 后处理糊掉）；取走粒子后画面必须逐位回到基线
    /// （空切片 = 一次 draw 都不发，不是"画了个透明的"）。
    #[test]
    fn particle_quads_light_up_the_frame() {
        let Some(mut shot) = gpu_or_skip("particle_quads_light_up_the_frame") else {
            return;
        };

        // 空场景 + 关天空：背景全黑，粒子的贡献没有第二解释。
        let mut scene = Scene::new("particles");
        scene.ambient = [0.0, 0.0, 0.0];
        let settings = sky_settings(false);
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, DEFAULT_FOV_Y);
        let (cx, cy) = (320 / 2, 240 / 2);

        let render = |quads: &[xgame_particle::ParticleQuad], shot: &mut ScreenshotRenderer| {
            shot.render_scene_with_particles(
                &scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                settings,
                quads,
            )
            .expect("渲染失败");
            pixel_avg(&shot.read_rgba().expect("读回失败"), 320, cx, cy)
        };

        // 背景 = HDR 目标的 clear 色（关天空后是一块深灰蓝，实测显示值 ≈ 97）：
        // 断言写成"与基线的差"，不写绝对值 —— 绝对阈值会在谁调 clear 色时碎掉。
        let baseline = render(&[], &mut shot);
        assert!(
            (60.0..140.0).contains(&baseline[0]) && (baseline[0] - baseline[2]).abs() < 20.0,
            "关天空 + 无实体的背景应当是均匀的深灰蓝: {baseline:?}"
        );

        // ① HDR 白粒子（size 1.0 世界单位盖住中心）：加色 + 曝光后应当饱和。
        let white = [xgame_particle::ParticleQuad {
            position: [0.0, 0.0, 0.0],
            size: 1.0,
            color: [4.0, 4.0, 4.0, 1.0],
            blend: xgame_core::ParticleBlend::Additive,
        }];
        let lit = render(&white, &mut shot);
        assert!(
            lit[0] > baseline[0] + 80.0 && lit[1] > baseline[1] + 80.0,
            "HDR 加色白粒子必须把中心显著提亮: {lit:?} vs 基线 {baseline:?}"
        );

        // ② 纯红粒子：红通道必须显著高于绿 / 蓝（颜色一路无损传到片元）。
        let red = [xgame_particle::ParticleQuad {
            position: [0.0, 0.0, 0.0],
            size: 1.0,
            color: [4.0, 0.0, 0.0, 1.0],
            blend: xgame_core::ParticleBlend::Additive,
        }];
        let tinted = render(&red, &mut shot);
        assert!(
            tinted[0] > tinted[1] + 60.0 && tinted[0] > tinted[2] + 60.0,
            "红色粒子的红通道必须压过绿 / 蓝: {tinted:?}"
        );
        assert!(
            tinted[0] > baseline[0] + 80.0,
            "红色粒子也必须把中心显著提亮: {tinted:?} vs {baseline:?}"
        );

        // ③ 取走粒子：逐位回到基线。
        let restored = render(&[], &mut shot);
        assert_eq!(restored, baseline, "没有粒子时必须与空场景逐位一致");
    }

    /// 深度语义：粒子**测试深度但不写深度**。
    ///
    /// 立方体挡在中间时，它背后的粒子必须被完全遮掉（画面与不放粒子逐位一致）；
    /// 挪到立方体前面则必须冒出来（加色让受光面变更亮）。这条钉子锁的是
    /// "粒子是场景里的东西"这条定位 —— 少了深度测试，火焰会从墙里透出来。
    #[test]
    fn particle_quads_respect_depth() {
        let Some(mut shot) = gpu_or_skip("particle_quads_respect_depth") else {
            return;
        };

        let mut scene = Scene::new("particle_depth");
        scene.ambient = [0.0, 0.0, 0.0];
        // 斜射光（含 -z 分量）：正对相机的 +z 面才受光。光要是垂直向下
        // （0,-1,0），照的是顶面，正面全黑，"粒子冒出来"就无从比起。
        scene.lights = vec![Light::directional("sun", [0.6, -1.0, -0.75], [1.0; 3], 1.0)];
        scene.lights[0].casts_shadow = false;
        let mut cube = Entity::new("Cube");
        cube.model = "builtin://cube".to_owned();
        scene.add_entity(cube);

        let settings = sky_settings(false);
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, DEFAULT_FOV_Y);
        let (cx, cy) = (320 / 2, 240 / 2);

        let render = |quads: &[xgame_particle::ParticleQuad], shot: &mut ScreenshotRenderer| {
            shot.render_scene_with_particles(
                &scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                settings,
                quads,
            )
            .expect("渲染失败");
            pixel_avg(&shot.read_rgba().expect("读回失败"), 320, cx, cy)
        };

        // 立方体正面在 z = 0.5：z = 0 的粒子在它背后，z = 1 的在它前面。
        let behind = [xgame_particle::ParticleQuad {
            position: [0.0, 0.0, 0.0],
            size: 1.0,
            color: [4.0, 4.0, 4.0, 1.0],
            blend: xgame_core::ParticleBlend::Additive,
        }];
        let mut front = behind;
        front[0].position = [0.0, 0.0, 1.0];

        let plain = render(&[], &mut shot);
        assert!(plain[0] > 100.0, "受光立方体正面必须亮: {plain:?}");

        let occluded = render(&behind, &mut shot);
        assert_eq!(
            occluded, plain,
            "被立方体挡住的粒子必须完全看不见（逐位等于无粒子）"
        );

        let in_front = render(&front, &mut shot);
        assert!(
            in_front[0] > plain[0] + 20.0,
            "立方体前面的粒子必须冒出来（加色提亮）: {in_front:?} vs {plain:?}"
        );
    }

    // -----------------------------------------------------------------
    // HDR / 色调映射（后处理链）：GPU 侧的"先证红"测试组
    // -----------------------------------------------------------------

    /// 白模立方体 + 一盏斜射、可调强度的方向光。正对相机的 +z 面受光，
    /// 采样点上的 N·L = |光传播方向归一化后的 z|。
    ///
    /// 斜光的意义：光与相机**不同轴** ⇒ 采样点落到 GGX 副瓣之外，"线性亮度
    /// = intensity × N·L"（π 校准确认，见 `scene_pipeline` 的 π 一节）的预测
    /// 才成立 —— HDR 与曝光测试的受控输入。
    fn tilted_lit_white_cube(intensity: f32) -> Scene {
        let mut scene = Scene::new("hdr");
        scene.ambient = [0.0, 0.0, 0.0];
        scene.lights = vec![Light::directional(
            "sun",
            [0.6, -1.0, -0.75],
            [1.0; 3],
            intensity,
        )];
        scene.lights[0].casts_shadow = false;
        let mut e = Entity::new("Cube");
        e.model = "builtin://cube".to_owned();
        scene.add_entity(e);
        scene
    }

    /// 受光面（+z 法线）上的漫反射线性亮度：`intensity × N·L`，白模 albedo = 1。
    fn front_face_linear(intensity: f32) -> f32 {
        let to_light = Vec3::new(-0.6, 1.0, 0.75).normalize();
        to_light.z * intensity
    }

    /// 与 CPU 镜像对表的容差（8 位台阶）：Rgba16Float 量化、MSAA resolve 与
    /// 硬件 sRGB 编码的合成误差远小于 2 个台阶，±4 是留了余量仍然有分辨力的界。
    const MIRROR_TOLERANCE: f32 = 4.0;

    /// **HDR 红线测试**：同一个高光（intensity 4），`Off` 模式裁成纯白（旧
    /// 管线语义），ACES 保留层次且像素值与 CPU 镜像对表。
    ///
    /// 这条测试在 HDR 路径缺席时必然红：场景直接画进 sRGB 目标的话，写入的
    /// 那一刻就钳到 1.0，"ACES 与 Off 的差异"根本无从谈起 —— 两个开关会
    /// 输出同一张 255 的图。
    #[test]
    fn aces_keeps_highlight_detail_that_off_clips() {
        let Some(mut shot) = gpu_or_skip("aces_keeps_highlight_detail_that_off_clips") else {
            return;
        };
        let scene = tilted_lit_white_cube(4.0);
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, DEFAULT_FOV_Y);
        let (cx, cy) = (320 / 2, 240 / 2);
        let render = |shot: &mut ScreenshotRenderer, settings: RenderConfig| {
            shot.render_scene(
                &scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                settings,
            )
            .expect("渲染失败");
            pixel_avg(&shot.read_rgba().expect("读回失败"), 320, cx, cy)
        };

        let aces = render(&mut shot, RenderConfig::default());
        let off = render(
            &mut shot,
            RenderConfig {
                tone_map: ToneMapMode::Off,
                ..Default::default()
            },
        );

        assert_eq!(
            off,
            [255.0, 255.0, 255.0],
            "Off 模式应当把 intensity 4 的高光裁成纯白（旧管线语义）"
        );
        assert!(
            aces[0] < 250.0,
            "ACES 下同样的高光必须明显离开顶点（滚降可见）: {aces:?}"
        );

        let expected = crate::postfx::cpu::to_srgb_u8(crate::postfx::cpu::tone_map(
            [front_face_linear(4.0); 3],
            RenderConfig::default(),
        ));
        for (got, want) in aces.iter().zip(expected) {
            assert!(
                (got - want as f32).abs() <= MIRROR_TOLERANCE,
                "ACES 像素应与 CPU 镜像对表: 实测 {aces:?} vs 预期 {expected:?}"
            );
        }
    }

    /// 曝光是**曲线之前**的亮度乘子，GPU 侧同样成立：0.25×/1×/4× 三档的
    /// 显示值严格递增（若哪天有人把曝光挪到曲线之后，高光端会被钳平，
    /// 这条测试立刻变红）。
    #[test]
    fn exposure_scales_highlights_before_the_curve_on_gpu() {
        let Some(mut shot) = gpu_or_skip("exposure_scales_highlights_before_the_curve_on_gpu")
        else {
            return;
        };
        let scene = tilted_lit_white_cube(4.0);
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, DEFAULT_FOV_Y);
        let (cx, cy) = (320 / 2, 240 / 2);
        let sample = |shot: &mut ScreenshotRenderer, exposure: f32| {
            let settings = RenderConfig {
                exposure,
                ..Default::default()
            };
            shot.render_scene(
                &scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                settings,
            )
            .expect("渲染失败");
            pixel_avg(&shot.read_rgba().expect("读回失败"), 320, cx, cy)[0]
        };

        let dark = sample(&mut shot, 0.25);
        let normal = sample(&mut shot, 1.0);
        let bright = sample(&mut shot, 4.0);
        assert!(
            dark < normal && normal < bright,
            "曝光 0.25×/1×/4× 的显示亮度必须严格递增: {dark} / {normal} / {bright}"
        );

        // 对表一档（曝光 0.25）：受光面线性 4.0×0.54 ≈ 2.16，曝光后 ≈ 0.54，
        // ACES 把它映到 ~200 的显示值（注意镜像函数自己会乘曝光 —— 传**未
        // 曝光**的线性值，别再预先乘一遍）。
        let expected = crate::postfx::cpu::to_srgb_u8(crate::postfx::cpu::tone_map(
            [front_face_linear(4.0); 3],
            RenderConfig {
                exposure: 0.25,
                ..Default::default()
            },
        ))[0];
        assert!(
            (dark - expected as f32).abs() <= MIRROR_TOLERANCE,
            "曝光 0.25× 的像素应与 CPU 镜像对表: 实测 {dark} vs 预期 {expected}"
        );
    }

    // -----------------------------------------------------------------
    // Bloom（后处理链第二片）：GPU 侧的"先证红"测试组
    // -----------------------------------------------------------------

    /// Bloom 开着的画面设置：只有 intensity 不同（阈值 / 软膝取默认）。
    fn bloom_settings(intensity: f32) -> RenderConfig {
        RenderConfig {
            bloom: BloomConfig {
                intensity,
                ..Default::default()
            },
            ..RenderConfig::default()
        }
    }

    /// **Bloom 红线测试**：高光块轮廓**之外**的像素在 Bloom 开启后必须被
    /// 显著提亮（光晕漏出），而远处角落不受影响（光晕围着亮源，不照亮全场）。
    ///
    /// 自证红的口径：第一条断言测的是"开与关的像素差" —— Bloom 链被整段
    /// 跳过（或提取/模糊里任一趟没跑）时同一点两档逐位相同，必红。
    #[test]
    fn bloom_glows_around_a_bright_spot() {
        let Some(mut shot) = gpu_or_skip("bloom_glows_around_a_bright_spot") else {
            return;
        };
        // 光强 8：正面线性亮度 ≈ 8×0.54 ≈ 4.3，远超阈值 1.0（提取量 ≈ 3.3）。
        let scene = tilted_lit_white_cube(8.0);
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, DEFAULT_FOV_Y);
        let (cx, cy) = (320 / 2, 240 / 2);
        // 正面在图上约 ±41px（0.5 世界单位在 3.5 深度上，45° 视场）。
        // near = 轮廓外 ~5px（光晕第一、二级的覆盖区）；far = 左上角落（>100px）。
        let near = (cx + 46, cy);
        let far = (10, 10);
        let render = |shot: &mut ScreenshotRenderer, settings: RenderConfig| {
            shot.render_scene(
                &scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                settings,
            )
            .expect("渲染失败");
            let rgba = shot.read_rgba().expect("读回失败");
            (
                pixel_avg(&rgba, 320, near.0, near.1),
                pixel_avg(&rgba, 320, far.0, far.1),
            )
        };

        let (off_near, off_far) = render(&mut shot, RenderConfig::default());
        // 两个采样点都落在背景上（背景是均匀清屏色）——不一致说明采样点
        // 打到立方体表面上了，后面的"变亮"就不是光晕的功劳。
        for ch in 0..3 {
            assert!(
                (off_near[ch] - off_far[ch]).abs() < 3.0,
                "背景不是匀色？near={off_near:?} far={off_far:?}（采样点打到物体上了？）"
            );
        }

        let (on_near, on_far) = render(&mut shot, bloom_settings(0.6));
        assert!(
            on_near[0] > off_near[0] + 25.0,
            "高光轮廓外的像素必须被光晕显著提亮：开 {on_near:?} vs 关 {off_near:?}"
        );
        assert!(
            (on_far[0] - off_far[0]).abs() < 4.0,
            "远处角落不该被光晕照亮：开 {on_far:?} vs 关 {off_far:?}"
        );

        // 强度单调：2.0 必须比 0.6 更亮（光晕加在曲线之前，ACES 单调）。
        let (strong_near, _) = render(&mut shot, bloom_settings(2.0));
        assert!(
            strong_near[0] > on_near[0] + 5.0,
            "强度 2.0 必须比 0.6 亮：{strong_near:?} vs {on_near:?}"
        );
    }

    /// **Bloom 的阈值红线**：低于软膝下限（threshold - knee = 0.4）的"微光"
    /// 一个光子都不提取。光强 0.35 的立方体正面线性 ≈ 0.19：开满 Bloom 也
    /// 不该在轮廓外留下光晕（若提取公式丢了阈值，这条会红）。
    #[test]
    fn bloom_leaves_sub_threshold_content_dark() {
        let Some(mut shot) = gpu_or_skip("bloom_leaves_sub_threshold_content_dark") else {
            return;
        };
        let scene = tilted_lit_white_cube(0.35);
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, DEFAULT_FOV_Y);
        let (cx, cy) = (320 / 2, 240 / 2);
        let near = (cx + 46, cy);
        let render = |shot: &mut ScreenshotRenderer, settings: RenderConfig| {
            shot.render_scene(
                &scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                settings,
            )
            .expect("渲染失败");
            pixel_avg(&shot.read_rgba().expect("读回失败"), 320, near.0, near.1)
        };

        let off = render(&mut shot, RenderConfig::default());
        let on = render(&mut shot, bloom_settings(0.6));
        for ch in 0..3 {
            assert!(
                (on[ch] - off[ch]).abs() < 3.0,
                "阈值以下的微光不该发光：开 {on:?} vs 关 {off:?}"
            );
        }
    }

    // -----------------------------------------------------------------
    // 实例化（draw batching）：GPU 侧的"先证红"测试组
    // -----------------------------------------------------------------
    //
    // 合批的三条合同：① 同图元合并成一批（CPU 读数看得见，画面看不出来）；
    // ② 合批不许改变画面（与逐 draw 渲染逐像素等价）；③ 逐实例数据（模型
    // 矩阵、材质）必须真的逐实例 —— 材质一旦被"提到批次级"，同批实体就会
    // 共用一份颜色，这是合批最典型的错法。阴影 pass 复用批次表，另有钉子。

    /// **合批计数**：64 个共用 `builtin://cube` 的实体 + 一块地面，必须压成
    /// 2 批。像素测试守得住"画得对"，守不住"是不是一批画完的"——旧实现
    /// （逐 drawcall）下这里必然是 65 批。
    #[test]
    fn instancing_batches_shared_mesh_entities() {
        let Some(mut shot) = gpu_or_skip("instancing_batches_shared_mesh_entities") else {
            return;
        };
        // 这条测的是**合批结构**（65 个绘制项压成 2 批），不是剔除；关掉剔除
        // 才能让"实例数 = 实体数"的口径保持纯粹。剔除的计数与画面等价由
        // `frustum_culling_*` 三条测试单独钉死。
        shot.set_frustum_culling(false);

        let mut scene = Scene::new("instancing-count");
        scene.ambient = [0.05; 3];
        scene.lights = vec![Light::directional("sun", [0.4, -1.0, -0.3], [1.0; 3], 1.5)];
        scene.lights[0].casts_shadow = false;

        let mut ground = Entity::new("Ground");
        ground.model = "builtin://plane".to_owned();
        ground.transform.scale = Vec3::new(9.0, 1.0, 9.0);
        scene.add_entity(ground);

        for i in 0..64 {
            let mut e = Entity::new(format!("c{i}"));
            e.model = "builtin://cube".to_owned();
            let (gx, gz) = ((i % 8) as f32, (i / 8) as f32);
            e.transform.position = Vec3::new((gx - 3.5) * 1.8, 0.5, (gz - 3.5) * 1.8);
            scene.add_entity(e);
        }

        // 斜上方俯视：别让实体互相遮挡导致采样点不好对（本测试不查像素，
        // 但画面照常渲染 —— 统计口径与真实帧一致）。
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 9.0, 10.0), Vec3::ZERO, DEFAULT_FOV_Y);
        shot.render_scene(
            &scene,
            &AnimationDriver::new(),
            Some(camera),
            SceneLighting::new(scene.ambient, &scene.lights),
            RenderConfig::default(),
        )
        .expect("渲染失败");

        let stats = shot.frame_stats();
        assert_eq!(stats.opaque_instances, 65, "65 个不透明绘制项");
        assert_eq!(
            stats.opaque_batches, 2,
            "地面 1 批 + 64 个立方体 1 批（旧实现 65 批）"
        );
        assert_eq!(stats.blended_draws, 0);
        assert_eq!(stats.draw_calls(), 2, "主 pass 的 draw call 数");
    }

    /// **逐 draw 画面等价**：几何完全相同，一边共用 `builtin://cube`（合批成
    /// 少批），一边 4 份各自注入的同几何网格（不合批），两图必须逐字节一致
    /// —— 合批只许省 draw call，不许改画面。开着阴影跑，把阴影 pass 的批次
    /// 复用也一起覆盖。
    #[test]
    fn instancing_matches_per_draw_rendering() {
        let Some(mut shot) = gpu_or_skip("instancing_matches_per_draw_rendering") else {
            return;
        };

        // 4 份独立网格：`from_mesh_data` 每次生成新的材质 id（各自的绑定组），
        // 同几何同参数 —— 正是"素材侧每个物体一份网格"的形态。
        let meshes: Vec<(String, GltfMesh)> = (0..4)
            .map(|i| {
                let builtin = crate::mesh::BuiltinMesh::Cube;
                let mesh = GltfMesh::from_mesh_data(
                    &shot.device,
                    &shot.queue,
                    &builtin.build(),
                    builtin.label(),
                );
                (format!("test://cube{i}"), mesh)
            })
            .collect();
        for (key, mesh) in meshes {
            shot.inject_model(key, mesh);
        }

        let make = |per_draw: bool| {
            let mut scene = Scene::new(if per_draw { "per-draw" } else { "shared" });
            scene.ambient = [0.05, 0.05, 0.08];
            scene.lights = vec![Light::directional("sun", [0.5, -1.0, -0.4], [1.0; 3], 2.0)];
            let mut ground = Entity::new("Ground");
            ground.model = "builtin://plane".to_owned();
            ground.transform.scale = Vec3::new(10.0, 1.0, 10.0);
            scene.add_entity(ground);
            for (i, p) in [
                (-1.5, 0.5, -0.5),
                (1.5, 0.5, -0.5),
                (-1.5, 0.5, 1.0),
                (1.5, 0.5, 1.0),
            ]
            .into_iter()
            .enumerate()
            {
                let mut e = Entity::new(format!("c{i}"));
                e.model = if per_draw {
                    format!("test://cube{i}")
                } else {
                    "builtin://cube".to_owned()
                };
                e.transform.position = Vec3::new(p.0, p.1, p.2);
                scene.add_entity(e);
            }
            scene
        };

        let camera = ShotCamera::looking_at(
            Vec3::new(0.0, 4.0, 6.0),
            Vec3::new(0.0, 0.0, 0.2),
            DEFAULT_FOV_Y,
        );
        let render = |shot: &mut ScreenshotRenderer, scene: &Scene| {
            shot.render_scene(
                scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .expect("渲染失败");
            shot.read_rgba().expect("读回失败")
        };

        let shared_scene = make(false);
        let shared = render(&mut shot, &shared_scene);
        let shared_stats = shot.frame_stats();
        let per_draw_scene = make(true);
        let per_draw = render(&mut shot, &per_draw_scene);
        let per_draw_stats = shot.frame_stats();

        assert!(
            shared_stats.opaque_batches < per_draw_stats.opaque_batches,
            "共用网格必须真的合批：共享 {} 批 vs 各自 {} 批",
            shared_stats.opaque_batches,
            per_draw_stats.opaque_batches
        );
        assert_eq!(
            shared_stats.opaque_instances, per_draw_stats.opaque_instances,
            "两种形态的绘制项数必须相同"
        );
        assert_eq!(shared.len(), per_draw.len());
        if shared != per_draw {
            let idx = shared
                .iter()
                .zip(&per_draw)
                .position(|(a, b)| a != b)
                .unwrap_or(0);
            panic!(
                "合批与逐 draw 的画面必须逐字节一致；首个差异在第 {idx} 字节（像素 {}）: {} vs {}",
                idx / 4,
                shared[idx],
                per_draw[idx]
            );
        }
    }

    /// **同批实体各调各的色**：两个共用网格的立方体各带红 / 绿实体级覆盖，
    /// 必须落在同一批（`opaque_batches == 1`），且两个采样点颜色各异 ——
    /// 钉死"材质随实例走"。材质若被提到批次级，同批实体只能共用一份颜色，
    /// 这条测试就是它的红。
    #[test]
    fn batching_keeps_per_instance_material_overrides() {
        let Some(mut shot) = gpu_or_skip("batching_keeps_per_instance_material_overrides") else {
            return;
        };

        let mut scene = Scene::new("per-instance-material");
        scene.ambient = [0.0; 3];
        // 斜射光 + 粗糙度 0.45：中心像素落在漫反射为主的位置（光与相机同轴
        // 时 GGX 副瓣会叠上约 0.24 线性白，把色调糊掉一半，见上一条测试）。
        scene.lights = vec![Light::directional("sun", [0.6, -1.0, -0.75], [1.0; 3], 1.0)];
        scene.lights[0].casts_shadow = false;

        let mut left = Entity::new("Left");
        left.model = "builtin://cube".to_owned();
        left.transform.position = Vec3::new(-0.7, 0.0, 0.0);
        left.material = Some(MaterialOverride {
            base_color: Some([0.9, 0.1, 0.1, 1.0]),
            ..Default::default()
        });
        scene.add_entity(left);
        let mut right = Entity::new("Right");
        right.model = "builtin://cube".to_owned();
        right.transform.position = Vec3::new(0.7, 0.0, 0.0);
        right.material = Some(MaterialOverride {
            base_color: Some([0.1, 0.9, 0.1, 1.0]),
            ..Default::default()
        });
        scene.add_entity(right);

        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, DEFAULT_FOV_Y);
        shot.render_scene(
            &scene,
            &AnimationDriver::new(),
            Some(camera),
            SceneLighting::new(scene.ambient, &scene.lights),
            RenderConfig::default(),
        )
        .expect("渲染失败");

        let stats = shot.frame_stats();
        assert_eq!(stats.opaque_batches, 1, "两个同网格实体应当合批");
        assert_eq!(stats.opaque_instances, 2);

        let rgba = shot.read_rgba().expect("读回失败");
        // 与 metallic/dielectric 测试同款采样几何：两个立方体中心投影在
        // 图像中心 ±58px。
        let (cx, cy) = (320 / 2, 240 / 2);
        let l = pixel_avg(&rgba, 320, cx - 58, cy);
        let r = pixel_avg(&rgba, 320, cx + 58, cy);
        assert!(l[0] > l[1] + 60.0, "左立方体应当偏红: {l:?}");
        assert!(r[1] > r[0] + 60.0, "右立方体应当偏绿: {r:?}");
    }

    /// **合批的阴影 pass 复用**：实例化批次表在阴影 pass 里原样复用（统计
    /// 口径不重复计 draw call），影子必须照常投出来。开 / 关阴影两图比对：
    /// 有一片像素变暗（影子）、没有任何像素反而变亮。
    #[test]
    fn instanced_draws_still_cast_shadows() {
        let Some(mut shot) = gpu_or_skip("instanced_draws_still_cast_shadows") else {
            return;
        };

        let mut scene = Scene::new("instanced-shadow");
        scene.ambient = [0.22, 0.22, 0.28];
        // 竖直向下的方向光：悬空立方体的影子正好落在正下方，位置可预测。
        scene.lights = vec![Light::directional("sun", [0.0, -1.0, 0.0], [1.0; 3], 1.6)];
        assert!(scene.lights[0].casts_shadow, "默认应当开阴影");
        let mut ground = Entity::new("Ground");
        ground.model = "builtin://plane".to_owned();
        ground.transform.scale = Vec3::new(12.0, 1.0, 12.0);
        scene.add_entity(ground);
        let mut cube = Entity::new("Cube");
        cube.model = "builtin://cube".to_owned();
        cube.transform.position = Vec3::new(0.0, 1.5, 0.0);
        scene.add_entity(cube);

        let camera = ShotCamera::looking_at(
            Vec3::new(0.0, 4.5, 7.0),
            Vec3::new(0.0, 0.3, 0.0),
            DEFAULT_FOV_Y,
        );
        let render = |shot: &mut ScreenshotRenderer, scene: &Scene| {
            shot.render_scene(
                scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .expect("渲染失败");
            shot.read_rgba().expect("读回失败")
        };

        let with_shadow = render(&mut shot, &scene);
        let stats = shot.frame_stats();
        scene.lights[0].casts_shadow = false;
        let without = render(&mut shot, &scene);

        let luma = |p: &[u8]| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
        let (mut darker, mut brighter) = (0usize, 0usize);
        let (pixels_a, _) = with_shadow.as_chunks::<4>();
        let (pixels_b, _) = without.as_chunks::<4>();
        for (a, b) in pixels_a.iter().zip(pixels_b) {
            let (la, lb) = (luma(a), luma(b));
            if la + 8.0 < lb {
                darker += 1;
            }
            if la > lb + 2.0 {
                brighter += 1;
            }
        }
        assert!(
            darker >= 100,
            "开阴影必须在地面上投出可见影子（变暗像素只有 {darker}）"
        );
        assert!(
            brighter <= 4,
            "开阴影不该让任何像素变亮（{brighter} 个变亮，可能是偏置漏光）"
        );
        // 阴影 pass 复用不透明批次：统计口径 = 主 pass 的 draw call 数。
        assert_eq!(stats.opaque_batches, 2, "地面 1 批 + 立方体 1 批");
        assert_eq!(stats.draw_calls(), 2);
    }

    /// **半透明不合批、远→近**：两个半透明立方体同轴（近蓝远红），中心像素
    /// 必须 b > r —— 蓝叠在红之上。绘制顺序若反了（近的先画），红的贡献压
    /// 在蓝之上，断言正好倒过来。
    #[test]
    fn blended_draws_render_far_to_near() {
        let Some(mut shot) = gpu_or_skip("blended_draws_render_far_to_near") else {
            return;
        };

        // 两块半透明网格：同几何、不同颜色（alpha 0.5），注入缓存（内置图元
        // 材质写死为不透明，需要受控输入）。粗糙度调高、金属度归零，压掉
        // 镜面白斑对色调的稀释。
        let meshes: Vec<(&str, GltfMesh)> = {
            let make = |color: [f32; 4]| {
                let builtin = crate::mesh::BuiltinMesh::Cube;
                let mut mesh = GltfMesh::from_mesh_data(
                    &shot.device,
                    &shot.queue,
                    &builtin.build(),
                    builtin.label(),
                );
                for prim in &mut mesh.primitives {
                    prim.material.base_color = color;
                    prim.material.blend = true;
                    prim.material.metallic = 0.0;
                    prim.material.roughness = 0.9;
                }
                mesh
            };
            vec![
                ("test://red", make([0.9, 0.05, 0.05, 0.5])),
                ("test://blue", make([0.05, 0.05, 0.9, 0.5])),
            ]
        };
        for (key, mesh) in meshes {
            shot.inject_model(key, mesh);
        }

        let mut scene = Scene::new("blend-order");
        scene.ambient = [0.0; 3];
        scene.lights = vec![Light::directional("sun", [0.6, -1.0, -0.75], [1.0; 3], 1.2)];
        scene.lights[0].casts_shadow = false;
        let mut far = Entity::new("FarRed");
        far.model = "test://red".to_owned();
        far.transform.position = Vec3::new(0.0, 0.0, -1.0);
        scene.add_entity(far);
        let mut near = Entity::new("NearBlue");
        near.model = "test://blue".to_owned();
        near.transform.position = Vec3::new(0.0, 0.0, 1.0);
        scene.add_entity(near);

        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, DEFAULT_FOV_Y);
        shot.render_scene(
            &scene,
            &AnimationDriver::new(),
            Some(camera),
            SceneLighting::new(scene.ambient, &scene.lights),
            RenderConfig::default(),
        )
        .expect("渲染失败");

        let stats = shot.frame_stats();
        assert_eq!(stats.opaque_batches, 0, "半透明不参与合批");
        assert_eq!(stats.blended_draws, 2, "半透明逐项一次 draw call");
        assert_eq!(stats.draw_calls(), 2);

        let rgba = shot.read_rgba().expect("读回失败");
        let center = pixel_avg(&rgba, 320, 160, 120);
        assert!(
            center[2] > center[0] + 30.0,
            "近处蓝色必须叠在远处红色之上（画反了会 r > b）: {center:?}"
        );
    }

    /// **FXAA**：1× MSAA 下斜边的硬台阶必须被磨出中间调；平坦区域不许被
    /// 无差别模糊（早期退出），否则"抗锯齿"就退化成"整幅柔焦"。
    ///
    /// 旧实现（没有 FXAA 这一趟）里开关只是个未接线的配置字段：开 / 关
    /// 两张图**逐字节相同**，第一条断言就红。这正是本测试的先证红。
    #[test]
    fn fxaa_smooths_edges_without_touching_flat_areas() {
        let Some(mut shot) = gpu_or_skip_msaa1("fxaa_smooths_edges_without_touching_flat_areas")
        else {
            return;
        };
        let scene = fxaa_probe_scene();
        let anim = AnimationDriver::new();
        let camera = ShotCamera::looking_at(Vec3::new(0.0, 0.0, 8.0), Vec3::ZERO, DEFAULT_FOV_Y);

        let mut render = |enabled: bool| {
            let settings = RenderConfig {
                fxaa: FxaaConfig { enabled },
                ..Default::default()
            };
            shot.render_scene(
                &scene,
                &anim,
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                settings,
            )
            .expect("渲染失败");
            shot.read_rgba().expect("读回失败")
        };

        let off = render(false);
        let on = render(true);
        assert_ne!(off, on, "FXAA 开关没接进后处理链：两张图逐字节相同");

        // 背景取左上角、前景取画面中心（薄盒正好压在视轴上）。
        let bg = luma_of(pixel_at(&off, 320, 4, 4));
        let fg = luma_of(pixel_at(&off, 320, 160, 120));
        assert!(
            (fg - bg).abs() > 0.15,
            "测试前提：前景 / 背景必须有足够对比度（bg={bg:.3} fg={fg:.3}）"
        );

        let steps_off = hard_luma_steps(&off, 320, 240, 0.25);
        let steps_on = hard_luma_steps(&on, 320, 240, 0.25);
        assert!(
            steps_off > 200,
            "测试前提：1× MSAA 的斜边必须有大量硬台阶（{steps_off}）"
        );
        assert!(
            steps_on < steps_off,
            "FXAA 应当把硬台阶磨成中间调：{steps_off} → {steps_on}"
        );
        assert!(
            steps_on * 10 <= steps_off * 9,
            "平滑幅度太小（{steps_off} → {steps_on}，不足 10%）—— 检查边缘检测阈值"
        );

        let mid_off = midtone_pixels(&off, 320, 240, bg, fg);
        let mid_on = midtone_pixels(&on, 320, 240, bg, fg);
        assert!(
            mid_on > mid_off,
            "FXAA 必须增加斜边上的中间调像素：{mid_off} → {mid_on}"
        );

        // 纯背景区域（远离轮廓）必须原样保留：5 抽头范围低于阈值 ⇒ 直接
        // 返回中心像素。整幅图都变 = 早期退出写错了。
        assert_eq!(
            pixel_at(&on, 320, 4, 4),
            pixel_at(&off, 320, 4, 4),
            "平坦背景不该被 FXAA 改动"
        );
    }

    // -----------------------------------------------------------------
    // 视锥剔除：GPU 侧的"先证红"测试组
    // -----------------------------------------------------------------
    //
    // 剔除的三条合同：① 视锥外的实体不产生实例（CPU 读数看得见，画面
    // 看不出来）；② 开 / 关剔除的画面**逐字节一致**（保守性）；③ 相机
    // 看不见、但影子落在画面里的物体必须继续参与阴影 pass（最容易漏的
    // 一条 —— 只按相机视锥剔，影子会跟着物体一起消失）。

    /// 16 个可见立方体（交错排布）+ 16 个相机背后的立方体。
    ///
    /// 交错是刻意的：剔除若只是"截断尾部"，实例槽位不会重排，像素等价
    /// 测试就测不出"逐实例数据是否跟着实例走"。这里可见项在开 / 关剔除
    /// 时的槽位号完全不同。
    fn interleaved_visible_and_hidden_cubes() -> Scene {
        let mut scene = Scene::new("culling");
        scene.ambient = [0.08; 3];
        scene.lights = vec![Light::directional("sun", [0.4, -1.0, -0.3], [1.0; 3], 1.2)];
        scene.lights[0].casts_shadow = false;

        for i in 0..32 {
            let mut e = Entity::new(format!("c{i}"));
            e.model = "builtin://cube".to_owned();
            if i % 2 == 0 {
                let g = (i / 2) as f32;
                e.transform.position =
                    Vec3::new((g % 4.0 - 1.5) * 0.7, (g / 4.0 - 1.5) * 0.7, -4.0);
            } else {
                // 相机背后：z 为正，任何朝向 -Z 的相机都看不见。
                let g = (i / 2) as f32;
                e.transform.position = Vec3::new(g % 4.0, g / 4.0, 3.0 + g * 0.05);
            }
            scene.add_entity(e);
        }
        scene
    }

    /// **剔除计数**：相机背后的 16 个实例必须没有进主 pass 的批次；关掉
    /// 剔除时它们全部进（32 个）。旧实现（无剔除）里 `culled_entities` 恒为
    /// 0、`opaque_instances` 恒为 32 —— 这条测试就是它的红。
    #[test]
    fn frustum_culling_skips_offscreen_instances() {
        let Some(mut shot) = gpu_or_skip("frustum_culling_skips_offscreen_instances") else {
            return;
        };
        let scene = interleaved_visible_and_hidden_cubes();
        let camera = ShotCamera::looking_at(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), DEFAULT_FOV_Y);
        let render = |shot: &mut ScreenshotRenderer| {
            shot.render_scene(
                &scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .expect("渲染失败");
            shot.frame_stats()
        };

        shot.set_frustum_culling(false);
        let unculled = render(&mut shot);
        assert_eq!(unculled.opaque_instances, 32, "关掉剔除：16 可见 + 16 隐藏");
        assert_eq!(unculled.culled_entities, 0);
        assert_eq!(unculled.culled_instances, 0);

        shot.set_frustum_culling(true);
        let culled = render(&mut shot);
        assert_eq!(culled.opaque_instances, 16, "只剩可见的 16 个实例");
        assert_eq!(culled.culled_entities, 16, "16 个整实体被跳过");
        assert_eq!(culled.culled_instances, 16);
        assert!(
            (culled.cull_ratio() - 0.5).abs() < 1e-6,
            "剔除比例应为五成：{}",
            culled.cull_ratio()
        );
        // 合批结构不受剔除影响：同图元仍是一批（只是少了实例）。
        assert_eq!(culled.opaque_batches, unculled.opaque_batches);
    }

    // =========================================================================
    // LOD（距离分级 + 绘制距离）
    // =========================================================================

    /// 一个立方体，顶点整体平移 / 缩放后作为 LOD 的某一级。
    ///
    /// 用**几何差别**而不是材质差别来区分各级：材质要走资产库（贴图 / `.mat`
    /// 都要落盘），而这里要证明的是"这个距离上画的是哪一份网格"。
    fn lod_cube(shot: &ScreenshotRenderer, key: &str, scale: f32, offset: Vec3) -> GltfMesh {
        let mut data = crate::mesh::BuiltinMesh::Cube.build();
        for vertex in data.vertices.iter_mut() {
            vertex.position = (Vec3::from(vertex.position) * scale + offset).to_array();
        }
        GltfMesh::from_mesh_data(&shot.device, &shot.queue, &data, key)
    }

    /// 与背景不同的像素数 + 它们的**水平质心**（像素坐标）。
    ///
    /// 背景是纯色清屏（天空默认关），所以"和左上角像素不一样"就是"画了东西"。
    /// 质心用来判断画的是哪一份网格 —— 只看像素数会被"大小不同但正好抵消"骗过。
    fn object_pixels(rgba: &[u8], width: u32, height: u32) -> (usize, f32) {
        let background = pixel_at(rgba, width, 0, 0);
        let mut count = 0usize;
        let mut sum_x = 0f64;
        for y in 0..height {
            for x in 0..width {
                let p = pixel_at(rgba, width, x, y);
                let diff = p
                    .iter()
                    .zip(background.iter())
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap_or(0);
                if diff > 6 {
                    count += 1;
                    sum_x += x as f64;
                }
            }
        }
        let centroid = if count == 0 {
            0.0
        } else {
            (sum_x / count as f64) as f32
        };
        (count, centroid)
    }

    /// 一台看向原点的相机，距离由 `distance` 给定。
    fn lod_camera(distance: f32) -> ShotCamera {
        ShotCamera::looking_at(Vec3::new(0.0, 0.0, distance), Vec3::ZERO, DEFAULT_FOV_Y)
    }

    fn lod_probe_scene(levels: Vec<xgame_core::LodLevel>) -> Scene {
        let mut scene = Scene::new("lod");
        // 纯环境光：不受光源方向影响，像素只由"画了哪份网格"决定。
        scene.ambient = [1.0; 3];
        scene.lights.clear();
        let mut entity = Entity::new("prop");
        entity.lod = xgame_core::LodGroup { levels };
        scene.add_entity(entity);
        scene
    }

    /// 距离变了要换**另一份网格**。
    ///
    /// 近处画 1 m 立方体（居中），远处画 6 m 立方体（整体右移 6 m）：
    /// 若实现"只换了计数、画面还画第 0 级"，质心不会动 —— 这条测试就是它的红。
    #[test]
    fn lod_switches_to_far_level_by_distance() {
        let Some(mut shot) = gpu_or_skip("lod_switches_to_far_level_by_distance") else {
            return;
        };
        let near = lod_cube(&shot, "lod-near", 1.0, Vec3::ZERO);
        let far = lod_cube(&shot, "lod-far", 6.0, Vec3::new(6.0, 0.0, 0.0));
        shot.inject_model("lod-near", near);
        shot.inject_model("lod-far", far);

        let scene = lod_probe_scene(vec![
            xgame_core::LodLevel::new("lod-near", 10.0),
            xgame_core::LodLevel::new("lod-far", 0.0),
        ]);

        let render = |shot: &mut ScreenshotRenderer, distance: f32| {
            shot.render_scene(
                &scene,
                &AnimationDriver::new(),
                Some(lod_camera(distance)),
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .expect("渲染失败");
            (shot.frame_stats(), shot.read_rgba().expect("读回失败"))
        };

        let (near_stats, near_rgba) = render(&mut shot, 5.0);
        assert_eq!(near_stats.low_lod_instances, 0, "5 米在阈值内：必须用第 0 级");
        assert_eq!(near_stats.distance_culled_entities, 0);
        assert_eq!(near_stats.opaque_instances, 1);
        let (near_count, near_center) = object_pixels(&near_rgba, 320, 240);
        assert!(near_count > 0, "近处必须画出来");

        let (far_stats, far_rgba) = render(&mut shot, 40.0);
        assert_eq!(far_stats.low_lod_instances, 1, "40 米必须换到第 1 级");
        assert_eq!(far_stats.distance_culled_entities, 0, "最后一级是无限远");
        let (far_count, far_center) = object_pixels(&far_rgba, 320, 240);
        assert!(far_count > 0, "远处必须画出来");

        // 第 1 级整体右移 6 米：40 米外的 6 米偏移 ≈ 50 像素。
        assert!(
            far_center - near_center > 20.0,
            "画面质心必须跟着换级移动：近 {near_center:.1} vs 远 {far_center:.1}"
        );
    }

    /// 超出最后一级的有限阈值 = **整实体不画**，而且**不投影**。
    ///
    /// 只做"主 pass 跳过、阴影照投"是最容易漏的一半：玩家会看到一团空气投下的
    /// 影子，比物体消失本身更难解释。
    #[test]
    fn lod_beyond_draw_distance_is_culled_and_casts_no_shadow() {
        let Some(mut shot) = gpu_or_skip("lod_beyond_draw_distance_is_culled_and_casts_no_shadow")
        else {
            return;
        };
        let near = lod_cube(&shot, "lod-near", 1.0, Vec3::ZERO);
        shot.inject_model("lod-near", near);

        let mut scene = lod_probe_scene(vec![xgame_core::LodLevel::new("lod-near", 10.0)]);
        scene.lights = vec![Light::directional("sun", [0.4, -1.0, -0.3], [1.0; 3], 3.0)];
        scene.lights[0].casts_shadow = true;

        shot.render_scene(
            &scene,
            &AnimationDriver::new(),
            Some(lod_camera(40.0)),
            SceneLighting::new(scene.ambient, &scene.lights),
            RenderConfig::default(),
        )
        .expect("渲染失败");

        let stats = shot.frame_stats();
        assert_eq!(stats.distance_culled_entities, 1, "40 米 > 10 米阈值：跳过");
        assert_eq!(stats.opaque_instances, 0, "主 pass 一个实例都不该有");
        assert_eq!(stats.culled_entities, 0, "这是距离剔除，不是视锥剔除");
        assert_eq!(stats.shadow_only_instances, 0, "被距离剔除的实体不许投影");
        let rgba = shot.read_rgba().expect("读回失败");
        assert_eq!(object_pixels(&rgba, 320, 240).0, 0, "画面里不该有它");
    }

    /// `lod_bias <= 0` = 关掉 LOD：同一份场景在 40 米外照样画第 0 级。
    #[test]
    fn lod_bias_zero_disables_switching_and_culling() {
        let Some(mut shot) = gpu_or_skip("lod_bias_zero_disables_switching_and_culling") else {
            return;
        };
        let near = lod_cube(&shot, "lod-near", 1.0, Vec3::ZERO);
        shot.inject_model("lod-near", near);

        let scene = lod_probe_scene(vec![xgame_core::LodLevel::new("lod-near", 10.0)]);
        let settings = RenderConfig {
            lod_bias: 0.0,
            ..RenderConfig::default()
        };
        shot.render_scene(
            &scene,
            &AnimationDriver::new(),
            Some(lod_camera(40.0)),
            SceneLighting::new(scene.ambient, &scene.lights),
            settings,
        )
        .expect("渲染失败");

        let stats = shot.frame_stats();
        assert_eq!(stats.distance_culled_entities, 0, "关掉 LOD 后不做距离剔除");
        assert_eq!(stats.opaque_instances, 1);
        assert_eq!(stats.low_lod_instances, 0, "永远是第 0 级");
        assert!(
            object_pixels(&shot.read_rgba().expect("读回失败"), 320, 240).0 > 0,
            "关掉 LOD 后远处也要画出来"
        );
    }

    // 参考图直接平移 CPU 顶点，不复用骨骼采样或槽位上传，避免同错假绿。
    fn dual_skin_probe(shadows: bool) {
        use crate::animation::{AnimClip, AnimNode, Interpolation, Skin, Track, TrackPath};
        use xgame_math::glam::Quat;

        let Some(mut shot) = gpu_or_skip("dual_skin_probe") else {
            assert!(
                std::env::var_os("XGAME_GPU_EVIDENCE_DIR").is_none(),
                "请求 GPU 证据时不允许跳过"
            );
            return;
        };
        let mut scene = Scene::new("dual-skin");
        scene.ambient = [0.22; 3];
        scene.lights = vec![Light::directional("sun", [0.0, -1.0, 0.0], [1.0; 3], 1.6)];
        scene.lights[0].casts_shadow = shadows;
        let mut ground = Entity::new("ground");
        ground.model = "builtin://plane".into();
        ground.transform.scale = Vec3::new(12.0, 1.0, 12.0);
        scene.add_entity(ground);
        for (i, count) in [2usize, 5].into_iter().enumerate() {
            let mut data = crate::mesh::BuiltinMesh::Cube.build();
            for v in &mut data.vertices {
                v.joints = [(count - 1) as f32, 0.0, 0.0, 0.0];
                v.weights = [1.0, 0.0, 0.0, 0.0];
            }
            let mut mesh = GltfMesh::from_mesh_data(&shot.device, &shot.queue, &data, "skin");
            mesh.primitives[0].skin = Some(0);
            mesh.nodes = (0..count)
                .map(|_| AnimNode {
                    parent: None,
                    name: String::new(),
                    translation: Vec3::ZERO,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                })
                .collect();
            mesh.skins = vec![Skin {
                joints: (0..count as u32).collect(),
                inverse_bind: vec![Mat4::IDENTITY; count],
            }];
            let sign = if i == 0 { 1.0 } else { -1.0 };
            mesh.animations = vec![AnimClip {
                name: "move".into(),
                duration: 1.0,
                tracks: vec![Track {
                    node: (count - 1) as u32,
                    path: TrackPath::Translation,
                    interpolation: Interpolation::Linear,
                    times: vec![0.0, 1.0],
                    values: vec![0.0, 0.0, 0.0, sign, 0.0, 0.0],
                }],
            }];
            // 两条路径使用相同保守盒，确保阴影拟合不会成为比较中的变量。
            mesh.bounds = Bounds {
                min: Vec3::splat(-2.0),
                max: Vec3::splat(2.0),
            };
            shot.inject_model(format!("skin-{i}"), mesh);
            let mut entity = Entity::new(format!("actor-{i}"));
            entity.model = format!("skin-{i}");
            entity.transform.position = Vec3::new(if i == 0 { -1.5 } else { 1.5 }, 1.5, 0.0);
            entity.animator = Some(xgame_core::Animator {
                clip: "move".into(),
                looping: false,
                ..Default::default()
            });
            scene.add_entity(entity);
        }
        let camera = ShotCamera::looking_at(
            Vec3::new(0.0, 5.0, 9.0),
            Vec3::new(0.0, 0.5, 0.0),
            DEFAULT_FOV_Y,
        );
        let render = |shot: &mut ScreenshotRenderer, scene: &Scene, anim: &AnimationDriver| {
            shot.render_scene(
                scene,
                anim,
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .unwrap();
            shot.read_rgba().unwrap()
        };
        let mut previous: Option<Vec<u8>> = None;
        for (frame, time) in [0.0, 0.5, 1.0, 0.0].into_iter().enumerate() {
            let mut anim = AnimationDriver::new();
            anim.tick(time, &scene.entities);
            let actual = render(&mut shot, &scene, &anim);
            assert_eq!(
                shot.frame_stats().opaque_instances,
                3,
                "两个角色与地面必须参与绘制"
            );
            if let Some(previous) = previous {
                assert!(actual != previous, "姿态变化必须改变像素");
            }
            previous = Some(actual.clone());
            if let Some(dir) = std::env::var_os("XGAME_GPU_EVIDENCE_DIR") {
                shot.save_png(
                    &PathBuf::from(dir).join(format!("dual-skin-shadow-{shadows}-{frame}.png")),
                )
                .unwrap();
            }
            let mut reference = scene.clone();
            for i in 0..2 {
                let mut data = crate::mesh::BuiltinMesh::Cube.build();
                for v in &mut data.vertices {
                    v.position[0] += if i == 0 { time } else { -time };
                }
                let mut mesh =
                    GltfMesh::from_mesh_data(&shot.device, &shot.queue, &data, "reference");
                mesh.bounds = Bounds {
                    min: Vec3::splat(-2.0),
                    max: Vec3::splat(2.0),
                };
                shot.inject_model(format!("reference-{i}"), mesh);
                reference.entities[i + 1].model = format!("reference-{i}");
                reference.entities[i + 1].animator = None;
            }
            let expected = render(&mut shot, &reference, &AnimationDriver::new());
            let different = actual
                .as_chunks::<4>()
                .0
                .iter()
                .zip(expected.as_chunks::<4>().0.iter())
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(
                different, 0,
                "双角色姿态必须等于独立 CPU 参考：shadow={shadows}, t={time}"
            );
            if shadows {
                reference.lights[0].casts_shadow = false;
                let unshadowed = render(&mut shot, &reference, &AnimationDriver::new());
                for side in 0..2 {
                    let dark = expected
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .zip(unshadowed.as_chunks::<4>().0.iter())
                        .enumerate()
                        .filter(|(p, (a, b))| {
                            p % 320 / 160 == side && a[0] as u16 + 8 < b[0] as u16
                        })
                        .count();
                    assert!(
                        dark > 50,
                        "每个角色都必须投出可见阴影：side={side}, pixels={dark}"
                    );
                }
            }
            // 交换实体顺序令 2/5 骨角色轮流占据第一个槽，且重用上一帧 GPU 缓冲。
            scene.entities.swap(1, 2);
            let reordered = render(&mut shot, &scene, &anim);
            assert!(actual == reordered, "槽位重排不能改变主画面或阴影");
            scene.entities.swap(1, 2);
        }
        eprintln!("dual_skin_probe shadows={shadows}: 4 poses, both slot orders, GPU pixel assertions executed");
    }

    #[test]
    fn dual_skin_dynamic_pixels_match_cpu_reference() {
        dual_skin_probe(false);
    }

    #[test]
    fn dual_skin_dynamic_shadows_match_cpu_reference() {
        dual_skin_probe(true);
    }

    fn load_morph_probe(shot: &ScreenshotRenderer, scale: f32) -> GltfMesh {
        static NEXT_PROBE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = NEXT_PROBE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "xgame-morph-bounds-{}-{serial}.gltf",
            std::process::id()
        ));
        std::fs::write(&path, crate::gltf_loader::tests::morph_gltf()).unwrap();
        let mesh = GltfMesh::from_file_with(
            &shot.device,
            &shot.queue,
            &path,
            xgame_core::ImportSettings {
                model_scale: scale,
                ..Default::default()
            },
        )
        .expect("合成 morph 资产应能加载");
        std::fs::remove_file(path).unwrap();
        mesh
    }

    #[test]
    fn morph_import_bounds_cover_combined_targets_and_scale() {
        let Some(shot) = gpu_or_skip("morph_import_bounds_cover_combined_targets_and_scale") else {
            return;
        };
        for scale in [1.0, 2.0] {
            let mesh = load_morph_probe(&shot, scale);
            assert!(mesh.bounds.max.x >= 1.1 * scale, "X 形变必须进入包围盒");
            assert!(mesh.bounds.max.y >= 1.2 * scale, "Y 形变必须进入包围盒");
            assert!(mesh.bounds.max.z >= 0.8 * scale, "两个目标同时启用必须累加");
        }
    }

    #[test]
    fn morph_culling_matches_unculled_pixels() {
        let Some(mut shot) = gpu_or_skip("morph_culling_matches_unculled_pixels") else {
            return;
        };
        let mesh = load_morph_probe(&shot, 1.0);
        shot.model_cache.insert("morph-probe".into(), mesh);
        let mut scene = Scene::new("morph-culling");
        scene.ambient = [1.0; 3];
        scene.lights.clear();
        let mut entity = Entity::new("face");
        entity.model = "morph-probe".into();
        // 绑定姿态位于左侧视锥外；形变将顶点向右和远处移动，进入画面。
        entity.transform.position = Vec3::new(-3.85, -0.5, -5.0);
        entity.transform.scale.z = -1.0;
        scene.add_entity(entity);
        let camera = ShotCamera::looking_at(Vec3::ZERO, -Vec3::Z, DEFAULT_FOV_Y);
        let render = |shot: &mut ScreenshotRenderer, scene: &Scene| {
            shot.render_scene(
                scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .unwrap();
            shot.read_rgba().unwrap()
        };
        shot.set_frustum_culling(false);
        scene.entities[0].morph_weights = vec![0.0, 0.0];
        let hidden = render(&mut shot, &scene);
        for weights in [vec![1.0, 1.0], vec![0.5, 1.0], vec![1.0, 1.0]] {
            scene.entities[0].morph_weights = weights;
            shot.set_frustum_culling(false);
            let reference = render(&mut shot, &scene);
            assert_ne!(reference, hidden, "形变后三角形必须真的进入画面");
            shot.set_frustum_culling(true);
            let culled = render(&mut shot, &scene);
            assert!(reference == culled, "形变后剔除开关必须逐像素一致");
        }
    }

    /// **像素等价**：开 / 关剔除的图像必须逐字节一致。剔除把"屏幕外的东西"
    /// 丢掉，屏幕上的像素没有任何理由变化；这条挂了说明剔除判据写错了
    /// （包围盒 / 视锥平面 / 槽位分配其中之一）。
    #[test]
    fn frustum_culling_matches_unculled_pixels() {
        let Some(mut shot) = gpu_or_skip("frustum_culling_matches_unculled_pixels") else {
            return;
        };
        let scene = interleaved_visible_and_hidden_cubes();
        let camera = ShotCamera::looking_at(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), DEFAULT_FOV_Y);
        let render = |shot: &mut ScreenshotRenderer| {
            shot.render_scene(
                &scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .expect("渲染失败");
            shot.read_rgba().expect("读回失败")
        };

        shot.set_frustum_culling(false);
        let unculled = render(&mut shot);
        shot.set_frustum_culling(true);
        let culled = render(&mut shot);

        assert_eq!(unculled.len(), culled.len());
        if unculled != culled {
            let idx = unculled
                .iter()
                .zip(&culled)
                .position(|(a, b)| a != b)
                .unwrap_or(0);
            panic!(
                "剔除不许改画面；首个差异在第 {idx} 字节（像素 {}）: {} vs {}",
                idx / 4,
                unculled[idx],
                culled[idx]
            );
        }
    }

    /// 多级联阴影的测试场景：地面 + 从近到远排开的方块。
    ///
    /// 深度跨度（约 34 个单位）刚好让 4 级级联各管一段 —— 每级只有一部分
    /// 方块在它的拟合框里，逐级剔除才有可剔的东西。全部挤在一小片区域里
    /// 会让"每级都能看见所有人"，那样的场景测不出任何差别。
    fn multi_cascade_shadow_scene() -> Scene {
        let mut scene = Scene::new("cascaded-shadow");
        scene.ambient = [0.05; 3];
        scene.lights = vec![Light::directional("sun", [0.45, -1.0, -0.35], [1.0; 3], 2.0)];
        assert!(scene.lights[0].casts_shadow);

        let mut ground = Entity::new("Ground");
        ground.model = "builtin://plane".to_owned();
        ground.transform.scale = Vec3::new(60.0, 1.0, 60.0);
        scene.add_entity(ground);

        for i in 0..12 {
            let mut e = Entity::new(format!("Caster{i}"));
            e.model = "builtin://cube".to_owned();
            let z = -3.0 - i as f32 * 2.6;
            e.transform.position = Vec3::new(((i % 3) as f32 - 1.0) * 2.2, 1.0, z);
            scene.add_entity(e);
        }
        scene
    }

    /// **像素等价**：阴影逐级剔除开 / 关的画面必须逐字节一致。
    ///
    /// 剔除只决定"某一级的深度图里要不要画这个实例"，而每一级的拟合框都**完全
    /// 罩住**它对应的子视锥 —— 框外的东西本来就会被裁掉。所以两档的画面没有
    /// 任何理由不同；这条挂了说明成员位判错（某一级漏了该投影的物体 → 影子缺
    /// 一块）或者段切分错位（画了别人的实例数据 → 影子错位）。
    #[test]
    fn cascaded_shadow_culling_matches_unculled_pixels() {
        let Some(mut shot) = gpu_or_skip("cascaded_shadow_culling_matches_unculled_pixels") else {
            return;
        };
        let scene = multi_cascade_shadow_scene();
        // 站在地面南侧略高处俯视：4 级级联铺满 z = -3 .. -34 这一段。
        let camera = ShotCamera::looking_at(
            Vec3::new(0.0, 7.0, 6.0),
            Vec3::new(0.0, 0.0, -16.0),
            DEFAULT_FOV_Y,
        );
        let render = |shot: &mut ScreenshotRenderer| {
            shot.render_scene(
                &scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .expect("渲染失败");
            shot.read_rgba().expect("读回失败")
        };

        shot.set_cascaded_shadow_culling(false);
        let unculled = render(&mut shot);
        let plain = shot.frame_stats();
        // 关掉时每级都画全部投影物：提交数必须是 4 ×（不透明 + 阴影专用）。
        let casters = plain.opaque_instances + plain.shadow_only_instances;
        assert!(casters > 0, "前提：场景里必须有投影物");
        assert_eq!(
            plain.shadow_instances_submitted,
            4 * casters,
            "关掉剔除时四级各画一遍全部投影物"
        );

        shot.set_cascaded_shadow_culling(true);
        let culled = render(&mut shot);
        let fast = shot.frame_stats();
        assert!(
            fast.shadow_instances_submitted < plain.shadow_instances_submitted,
            "剔除必须真的少画：{} vs {}",
            fast.shadow_instances_submitted,
            plain.shadow_instances_submitted
        );

        assert_eq!(unculled.len(), culled.len());
        if unculled != culled {
            let idx = unculled
                .iter()
                .zip(&culled)
                .position(|(a, b)| a != b)
                .unwrap_or(0);
            panic!(
                "逐级剔除不许改画面；首个差异在第 {idx} 字节（像素 {}）: {} vs {}",
                idx / 4,
                unculled[idx],
                culled[idx]
            );
        }
    }

    /// **阴影补投**：相机视锥外、但影子落在画面里的投影物必须继续参与
    /// 阴影 pass。
    ///
    /// 机位 (0, 14, 18) 俯视原点，立方体在 (-12, 5, 0) —— 水平角 33.7°
    /// 超出 4:3 / 45° 视场的 28.9° 半角，肯定不在主 pass。光 (1,-1,0)
    /// 把它的影子推到 (-7, 0, 0)，正落在可见地面上。旧实现里这个实体
    /// 在收集阶段就被整段跳过，阴影会凭空消失 —— 这条测试就是它的红。
    #[test]
    fn offscreen_casters_still_cast_shadows() {
        let Some(mut shot) = gpu_or_skip("offscreen_casters_still_cast_shadows") else {
            return;
        };

        let mut scene = Scene::new("shadow-only-caster");
        scene.ambient = [0.05; 3];
        scene.lights = vec![Light::directional("sun", [1.0, -1.0, 0.0], [1.0; 3], 2.0)];
        assert!(scene.lights[0].casts_shadow);
        let mut ground = Entity::new("Ground");
        ground.model = "builtin://plane".to_owned();
        ground.transform.scale = Vec3::new(30.0, 1.0, 30.0);
        scene.add_entity(ground);
        let mut caster = Entity::new("Caster");
        caster.model = "builtin://cube".to_owned();
        caster.transform.position = Vec3::new(-12.0, 5.0, 0.0);
        scene.add_entity(caster);

        let camera = ShotCamera::looking_at(Vec3::new(0.0, 14.0, 18.0), Vec3::ZERO, DEFAULT_FOV_Y);
        let render = |shot: &mut ScreenshotRenderer, scene: &Scene| {
            shot.render_scene(
                scene,
                &AnimationDriver::new(),
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .expect("渲染失败");
            shot.read_rgba().expect("读回失败")
        };

        // 先把模型装进缓存，才能用同一套取景参数算出投影坐标。
        let _ = render(&mut shot, &scene);
        let world = scene.world_matrices();
        let bounds = scene_bounds(&scene.entities, &world, &shot.model_cache).expect("场景有几何");
        let (near, far) = clip_planes(&camera, bounds.radius());
        let view_proj = Mat4::perspective_rh(camera.fov_y, shot.aspect(), near, far)
            * Mat4::look_at_rh(camera.eye, camera.target, camera.up);
        let project = |p: Vec3| {
            let clip = view_proj * p.extend(1.0);
            clip.truncate() / clip.w
        };

        let caster_ndc = project(Vec3::new(-12.0, 5.0, 0.0));
        assert!(
            caster_ndc.x.abs() > 1.0,
            "前提：投影物必须在相机视锥外（ndc.x = {}）",
            caster_ndc.x
        );

        let shadow_ndc = project(Vec3::new(-7.0, 0.0, 0.0));
        let sx = ((shadow_ndc.x * 0.5 + 0.5) * 320.0).round() as u32;
        let sy = ((1.0 - (shadow_ndc.y * 0.5 + 0.5)) * 240.0).round() as u32;
        assert!(sx < 320 && sy < 240, "影子落点必须在画面内: ({sx}, {sy})");

        let with_shadow = render(&mut shot, &scene);
        let stats = shot.frame_stats();
        assert_eq!(stats.opaque_instances, 1, "主 pass 只剩地面");
        assert_eq!(
            stats.shadow_only_instances, 1,
            "相机外的投影物必须进阴影专用批次"
        );
        assert_eq!(stats.culled_entities, 0, "投影物不能算被剔除");

        scene.lights[0].casts_shadow = false;
        let without_shadow = render(&mut shot, &scene);

        let luma = |p: [f32; 3]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
        let lit = luma(pixel_avg(&with_shadow, 320, sx, sy));
        let unlit = luma(pixel_avg(&without_shadow, 320, sx, sy));
        assert!(
            lit + 20.0 < unlit,
            "屏幕外投影物的影子必须落在地面上：有阴影 {lit:.1} vs 无阴影 {unlit:.1}"
        );
    }

    // -----------------------------------------------------------------
    // 点光立方体阴影：GPU 侧像素测试
    // -----------------------------------------------------------------

    /// 地板 + 两只分居 ±X 的箱子 + 一盏吊在正上方的点光。
    ///
    /// 两只箱子刻意错开在相反方向：立方体的 `+X` 与 `-X` 是两张不同的面，
    /// 面序 / 矩阵配对写错时**只有一侧**会没有影子，另一侧完全正常 —— 单只
    /// 箱子的用例看不出这种错。
    fn point_shadow_probe_scene(point_shadow: bool) -> Scene {
        let mut scene = Scene::new("point-shadow");
        scene.ambient = [0.02; 3];
        let mut lamp = Light::point("lamp", [0.0, 7.0, 0.0], [1.0; 3], 6.0, 40.0);
        lamp.casts_shadow = point_shadow;
        scene.lights = vec![lamp];
        let mut floor = Entity::new("Floor");
        floor.model = "builtin://plane".to_owned();
        floor.transform.scale = Vec3::new(30.0, 1.0, 30.0);
        scene.add_entity(floor);
        for (i, x) in [-4.0f32, 4.0].into_iter().enumerate() {
            let mut caster = Entity::new(format!("Caster{i}"));
            caster.model = "builtin://cube".to_owned();
            caster.transform.position = Vec3::new(x, 2.0, 0.0);
            caster.transform.scale = Vec3::splat(2.0);
            scene.add_entity(caster);
        }
        scene
    }

    /// 点光的影子必须落在**几何上正确的方向**，且不能把没被遮住的地方也压暗。
    ///
    /// 三个采样点：
    /// - 两处影子中心（±4.6, 0, 0）：开阴影后必须明显变暗；
    /// - 一处对照（0, 0, 0，正好在灯下面却不在任何箱子的影子里）：必须不变 ——
    ///   这条挡住"参考深度尺度错 → 整块地板全判为阴影"这类错误（那也会让前两条
    ///   变暗通过，假绿）。
    #[test]
    fn point_light_shadows_land_on_the_floor() {
        let Some(mut shot) = gpu_or_skip("point_light_shadows_land_on_the_floor") else {
            return;
        };
        let anim = AnimationDriver::new();
        let camera = ShotCamera::looking_at(
            Vec3::new(0.0, 9.0, 16.0),
            Vec3::new(0.0, 0.0, 0.0),
            DEFAULT_FOV_Y,
        );

        let on_scene = point_shadow_probe_scene(true);
        let with_shadow = render_frames(&mut shot, &on_scene, &anim, camera, RenderConfig::default(), 1);
        let stats = shot.frame_stats();
        // 3 个实例（地板 + 两只箱子）× 6 面 —— 少一面就少 3。
        assert_eq!(
            stats.point_shadow_instances, 18,
            "六个面都要把全部投影物画一遍（实际 {}）",
            stats.point_shadow_instances
        );
        // 每面一遍：draw 次数 = 6 × 批次表长度（合批之后地板 1 批、两只箱子 1 批）。
        let batches = stats.opaque_batches + stats.shadow_only_batches;
        assert_eq!(
            stats.point_shadow_draw_calls,
            batches * crate::scene_pipeline::POINT_SHADOW_FACES,
            "六面各画一遍全部批次（批次 {batches}）"
        );

        let off_scene = point_shadow_probe_scene(false);
        let without_shadow =
            render_frames(&mut shot, &off_scene, &anim, camera, RenderConfig::default(), 1);
        assert_eq!(
            shot.frame_stats().point_shadow_instances,
            0,
            "没开点光阴影时不该跑立方体 pass"
        );

        // 投影用的是与渲染同一套取景参数（模型已进缓存，`scene_bounds` 可用）。
        let world = on_scene.world_matrices();
        let bounds = scene_bounds(&on_scene.entities, &world, &shot.model_cache).expect("场景有几何");
        let (near, far) = clip_planes(&camera, bounds.radius());
        let view_proj = Mat4::perspective_rh(camera.fov_y, shot.aspect(), near, far)
            * Mat4::look_at_rh(camera.eye, camera.target, camera.up);
        let to_pixel = |p: Vec3| {
            let clip = view_proj * p.extend(1.0);
            let ndc = clip.truncate() / clip.w;
            (
                ((ndc.x * 0.5 + 0.5) * 320.0).round() as u32,
                ((1.0 - (ndc.y * 0.5 + 0.5)) * 240.0).round() as u32,
            )
        };
        let luma = |p: [f32; 3]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];

        for x in [-4.6f32, 4.6] {
            let (px, py) = to_pixel(Vec3::new(x, 0.0, 0.0));
            assert!(px < 320 && py < 240, "影子落点必须在画面内: ({px}, {py})");
            let lit = luma(pixel_avg(&with_shadow, 320, px, py));
            let unlit = luma(pixel_avg(&without_shadow, 320, px, py));
            assert!(
                lit + 20.0 < unlit,
                "x = {x} 处的影子必须落在地面上：有阴影 {lit:.1} vs 无阴影 {unlit:.1}"
            );
        }

        let (cx, cy) = to_pixel(Vec3::ZERO);
        let lit_ctrl = luma(pixel_avg(&with_shadow, 320, cx, cy));
        let unlit_ctrl = luma(pixel_avg(&without_shadow, 320, cx, cy));
        assert!(
            (lit_ctrl - unlit_ctrl).abs() < 8.0,
            "没有被遮住的地方不该变暗：有阴影 {lit_ctrl:.1} vs 无阴影 {unlit_ctrl:.1}"
        );
    }

    /// 影子落在**竖直挡板**上时，位置必须正确（不只是"某处有个影子"）。
    ///
    /// 灯放在原点、挡板在 `+X` 侧、箱子刻意偏在 `+Z` 一侧：影子必须落在
    /// `+Z` 那半边。立方体的水平轴（`u`）一旦镜像，影子会**原样翻到 `-Z` 侧**
    /// —— 形状、大小、软硬都完全正常，只看"有没有影子"永远发现不了。
    /// 这一条同时钉住参考深度用**主轴分量**（第一版用欧氏长度时整面墙都是黑的）。
    #[test]
    fn point_shadow_lands_on_the_correct_side_of_a_wall() {
        let Some(mut shot) = gpu_or_skip("point_shadow_lands_on_the_correct_side_of_a_wall") else {
            return;
        };
        let build = |shadows: bool| {
            let mut scene = Scene::new("point-shadow-wall");
            scene.ambient = [0.02; 3];
            // 光在原点、挡板内表面在 x = 5.75：光线几乎垂直打在挡板上，
            // 避开掠射角的自遮挡（那是偏置问题，会把"位置对不对"淹掉）。
            let mut lamp = Light::point("lamp", [0.0, 0.0, 0.0], [1.0; 3], 12.0, 30.0);
            lamp.casts_shadow = shadows;
            scene.lights = vec![lamp];
            let mut wall = Entity::new("Wall");
            wall.model = "builtin://cube".to_owned();
            wall.transform.position = Vec3::new(6.0, 0.0, 0.0);
            wall.transform.scale = Vec3::new(0.5, 10.0, 10.0);
            scene.add_entity(wall);
            let mut caster = Entity::new("Caster");
            caster.model = "builtin://cube".to_owned();
            caster.transform.position = Vec3::new(3.0, 0.0, 1.2);
            caster.transform.scale = Vec3::splat(0.8);
            scene.add_entity(caster);
            scene
        };
        let anim = AnimationDriver::new();
        let camera = ShotCamera::looking_at(
            Vec3::new(1.0, 1.0, 14.0),
            Vec3::new(6.0, 0.0, 0.0),
            DEFAULT_FOV_Y,
        );

        let on_scene = build(true);
        let with_shadow = render_frames(&mut shot, &on_scene, &anim, camera, RenderConfig::default(), 1);
        let off_scene = build(false);
        let without_shadow =
            render_frames(&mut shot, &off_scene, &anim, camera, RenderConfig::default(), 1);

        let world = on_scene.world_matrices();
        let bounds = scene_bounds(&on_scene.entities, &world, &shot.model_cache).expect("场景有几何");
        let (near, far) = clip_planes(&camera, bounds.radius());
        let view_proj = Mat4::perspective_rh(camera.fov_y, shot.aspect(), near, far)
            * Mat4::look_at_rh(camera.eye, camera.target, camera.up);
        let to_pixel = |p: Vec3| {
            let clip = view_proj * p.extend(1.0);
            let ndc = clip.truncate() / clip.w;
            (
                ((ndc.x * 0.5 + 0.5) * 320.0).round() as u32,
                ((1.0 - (ndc.y * 0.5 + 0.5)) * 240.0).round() as u32,
            )
        };
        let luma = |p: [f32; 3]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
        let sample = |rgba: &[u8], p: Vec3| {
            let (px, py) = to_pixel(p);
            assert!(px < 320 && py < 240, "采样点必须在画面内: ({px}, {py})");
            luma(pixel_avg(rgba, 320, px, py))
        };

        // 箱子中心 (3, 0, 1.2) → 光线打到 x = 5.75 的落点 (5.75, 0, 2.4)。
        let shadow = sample(&with_shadow, Vec3::new(5.75, 0.0, 2.4));
        let lit = sample(&without_shadow, Vec3::new(5.75, 0.0, 2.4));
        assert!(
            shadow + 20.0 < lit,
            "影子必须落在箱子那一侧（z = +2.4）：有阴影 {shadow:.1} vs 无阴影 {lit:.1}"
        );

        // 镜像位置必须**不受影响**（u 轴镜像时它才是暗的那一侧）。
        let mirrored = sample(&with_shadow, Vec3::new(5.75, 0.0, -2.4));
        let mirrored_lit = sample(&without_shadow, Vec3::new(5.75, 0.0, -2.4));
        assert!(
            (mirrored - mirrored_lit).abs() < 8.0,
            "镜像位置不该有影子（有 {mirrored:.1} vs 无 {mirrored_lit:.1}）"
        );
    }

    /// 点光阴影开着时，两次全新渲染器的结果必须逐字节一致（无随机相位）。
    #[test]
    fn point_shadow_is_deterministic_across_runs() {
        let Some(mut shot) = gpu_or_skip("point_shadow_is_deterministic_across_runs") else {
            return;
        };
        let anim = AnimationDriver::new();
        let camera = ShotCamera::looking_at(
            Vec3::new(0.0, 9.0, 16.0),
            Vec3::new(0.0, 0.0, 0.0),
            DEFAULT_FOV_Y,
        );
        let scene = point_shadow_probe_scene(true);
        let a = render_frames(&mut shot, &scene, &anim, camera, RenderConfig::default(), 1);
        let b = render_frames(&mut shot, &scene, &anim, camera, RenderConfig::default(), 1);
        assert_eq!(a, b, "同一渲染器连渲两帧必须逐字节一致");
    }

    // =========================================================================
    // 蒙皮验收：多关节权重混合 / 骨槽容量超限 / 真实 glTF
    // =========================================================================

    /// 纯平移骨架的立方体：骨 `k` 在 t=1 时刻平移到 `moves[k].1`。
    ///
    /// 每个顶点的影响由 `assign(i)` 给定（下标 + 权重）。偏移取 2 的幂次、
    /// 权重取二进制分数 —— CPU 参考与 GPU 的所有乘加都精确可表示，
    /// 逐像素比较不需要给浮点容差留后门。
    fn skinned_cube(
        shot: &ScreenshotRenderer,
        key: &str,
        joint_count: u32,
        assign: impl Fn(usize) -> ([f32; 4], [f32; 4]),
        moves: &[(u32, Vec3)],
    ) -> GltfMesh {
        use crate::animation::{AnimClip, AnimNode, Interpolation, Skin, Track, TrackPath};
        use xgame_math::glam::Quat;

        let mut data = crate::mesh::BuiltinMesh::Cube.build();
        for (i, v) in data.vertices.iter_mut().enumerate() {
            let (joints, weights) = assign(i);
            v.joints = joints;
            v.weights = weights;
        }
        let mut mesh = GltfMesh::from_mesh_data(&shot.device, &shot.queue, &data, key);
        mesh.primitives[0].skin = Some(0);
        mesh.nodes = (0..joint_count)
            .map(|_| AnimNode {
                parent: None,
                name: String::new(),
                translation: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            })
            .collect();
        mesh.skins = vec![Skin {
            joints: (0..joint_count).collect(),
            inverse_bind: vec![Mat4::IDENTITY; joint_count as usize],
        }];
        mesh.animations = vec![AnimClip {
            name: "move".into(),
            duration: 1.0,
            tracks: moves
                .iter()
                .map(|(node, offset)| Track {
                    node: *node,
                    path: TrackPath::Translation,
                    interpolation: Interpolation::Linear,
                    times: vec![0.0, 1.0],
                    values: vec![0.0, 0.0, 0.0, offset.x, offset.y, offset.z],
                })
                .collect(),
        }];
        // 两条路径使用相同保守盒，阴影拟合 / 取景不会成为比较中的变量。
        mesh.bounds = Bounds {
            min: Vec3::splat(-2.0),
            max: Vec3::splat(2.0),
        };
        mesh
    }

    /// 静态参考网格（纯平移烘焙）：顶点位置加 `delta`，不带蒙皮。
    fn baked_cube(shot: &ScreenshotRenderer, key: &str, delta: impl Fn(usize) -> Vec3) -> GltfMesh {
        let mut data = crate::mesh::BuiltinMesh::Cube.build();
        for (i, v) in data.vertices.iter_mut().enumerate() {
            v.position = (Vec3::from(v.position) + delta(i)).to_array();
        }
        let mut mesh = GltfMesh::from_mesh_data(&shot.device, &shot.queue, &data, key);
        mesh.bounds = Bounds {
            min: Vec3::splat(-2.0),
            max: Vec3::splat(2.0),
        };
        mesh
    }

    fn pixel_differences(a: &[u8], b: &[u8]) -> usize {
        a.as_chunks::<4>()
            .0
            .iter()
            .zip(b.as_chunks::<4>().0.iter())
            .filter(|(x, y)| x != y)
            .count()
    }

    /// 逐像素差异诊断：`(精确差异像素数, 最大单通道差, 超过 2/255 的像素数)`。
    ///
    /// GPU 与 CPU 的 f32 求值顺序可能差一个 ULP，经光照放大后落在 8-bit
    /// 输出上就是 ±1 的碎差；真 bug（骨骼用错 / 只取一根骨）则会产生大面积
    /// 显著差异。这个三元组把"噪声"与"错误"分开。
    fn pixel_diff_stats(a: &[u8], b: &[u8]) -> (usize, u8, usize) {
        let mut exact = 0usize;
        let mut max_delta = 0u8;
        let mut over_two = 0usize;
        for (x, y) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0.iter()) {
            if x == y {
                continue;
            }
            exact += 1;
            let delta = x
                .iter()
                .zip(y.iter())
                .map(|(c, d)| c.abs_diff(*d))
                .max()
                .unwrap_or(0);
            max_delta = max_delta.max(delta);
            if delta > 2 {
                over_two += 1;
            }
        }
        (exact, max_delta, over_two)
    }

    /// **多关节权重混合**（R03/031 验收）：一个顶点可同时受最多 4 根骨影响，
    /// 按 `skin = (Σ wₖ·boneₖ) / Σwₖ` 线性混合。此前的 GPU 回归每顶点只绑
    /// 一根骨（权重 `[1,0,0,0]`），着色器的 `w.y / w.z / w.w` 分支从未被
    /// 渲染路径走过 —— 这条把混合公式本身钉在逐像素一致上。
    ///
    /// 覆盖四种模式：4 骨分数权重、双骨、**权重和为 2**（归一化分支）、
    /// 同一骨重复 4 次。
    #[test]
    fn multi_joint_skin_blending_matches_cpu_reference() {
        let Some(mut shot) = gpu_or_skip("multi_joint_skin_blending_matches_cpu_reference") else {
            assert!(
                std::env::var_os("XGAME_GPU_EVIDENCE_DIR").is_none(),
                "请求 GPU 证据时不允许跳过"
            );
            return;
        };
        let offsets = [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.5, 0.0),
            Vec3::new(0.0, 0.0, 0.25),
            Vec3::new(-0.5, 0.0, 0.0),
        ];
        let patterns: [([f32; 4], [f32; 4]); 4] = [
            ([0.0, 1.0, 2.0, 3.0], [0.5, 0.25, 0.125, 0.125]),
            ([0.0, 1.0, 0.0, 0.0], [0.5, 0.5, 0.0, 0.0]),
            ([2.0, 3.0, 0.0, 0.0], [1.0, 1.0, 0.0, 0.0]),
            ([1.0, 1.0, 1.0, 1.0], [0.25, 0.25, 0.25, 0.25]),
        ];
        let moves = [
            (0, offsets[0]),
            (1, offsets[1]),
            (2, offsets[2]),
            (3, offsets[3]),
        ];
        let mesh = skinned_cube(&shot, "multi-joint", 4, |i| patterns[i % 4], &moves);
        shot.inject_model("multi-joint", mesh);
        let mut scene = Scene::new("multi-joint");
        scene.ambient = [0.22; 3];
        scene.lights = vec![Light::directional("sun", [0.0, -1.0, 0.0], [1.0; 3], 1.6)];
        scene.lights[0].casts_shadow = false;
        let mut actor = Entity::new("actor");
        actor.model = "multi-joint".into();
        actor.transform.position = Vec3::new(0.0, 1.5, 0.0);
        actor.animator = Some(xgame_core::Animator {
            clip: "move".into(),
            looping: false,
            ..Default::default()
        });
        scene.add_entity(actor);
        let camera = ShotCamera::looking_at(
            Vec3::new(0.0, 5.0, 9.0),
            Vec3::new(0.0, 1.0, 0.0),
            DEFAULT_FOV_Y,
        );
        let render = |shot: &mut ScreenshotRenderer, scene: &Scene, anim: &AnimationDriver| {
            shot.render_scene(
                scene,
                anim,
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .unwrap();
            shot.read_rgba().unwrap()
        };

        let mut previous: Option<Vec<u8>> = None;
        for time in [0.0, 0.5, 1.0, 0.0] {
            let mut anim = AnimationDriver::new();
            anim.tick(time, &scene.entities);
            let actual = render(&mut shot, &scene, &anim);
            assert_eq!(shot.frame_stats().opaque_instances, 1, "角色必须参与绘制");
            if let Some(previous) = previous.as_ref() {
                assert!(actual != *previous, "姿态变化必须改变像素");
            }
            previous = Some(actual.clone());

            // CPU 参考：纯平移骨架下 skin·v = v + Σ (wₖ/Σw)·tₖ。
            let mut reference = scene.clone();
            let baked = baked_cube(&shot, "multi-joint-ref", |i| {
                let (joints, weights) = patterns[i % 4];
                let wsum: f32 = weights.iter().sum();
                let mut delta = Vec3::ZERO;
                for k in 0..4 {
                    delta += offsets[joints[k] as usize] * (weights[k] / wsum);
                }
                delta * time
            });
            shot.inject_model("multi-joint-ref", baked);
            reference.entities[0].model = "multi-joint-ref".into();
            reference.entities[0].animator = None;
            let expected = render(&mut shot, &reference, &AnimationDriver::new());
            let different = pixel_differences(&actual, &expected);
            assert_eq!(
                different, 0,
                "多关节混合必须与独立 CPU 参考逐像素一致：t={time}"
            );
        }
    }

    /// **骨槽容量超限的顶点影响必须清零，不许读进下一个实体的槽**
    /// （R03/031 的"固定容量超限策略"）。
    ///
    /// 骨架超过 256 骨时矩阵上传会截断到槽内，但顶点上的关节索引若仍指向
    /// 256 以外，着色器的 `base + index` 会读进**下一个实体的槽** —— 邻居的
    /// 骨骼运动会泄漏进这个角色（画面症状：别人一动它跟着变形）。
    ///
    /// 场景：A 是 300 骨三角形（**走真实导入路径**：`from_file_with` 加载
    /// `oversized_skin_gltf()`，顶点影响跨在 256 边界上），B 是 2 骨玩具。
    /// 修复后：索引 ≥ 256 的影响清零（shader 的 `1/wsum` 把剩下的重归一），
    /// 半越界的顶点只跟骨 255 动、全越界的顶点冻结在绑定姿态；B 照常。
    #[test]
    fn oversized_skin_influences_freeze_instead_of_leaking_into_the_next_slot() {
        let Some(mut shot) =
            gpu_or_skip("oversized_skin_influences_freeze_instead_of_leaking_into_the_next_slot")
        else {
            assert!(
                std::env::var_os("XGAME_GPU_EVIDENCE_DIR").is_none(),
                "请求 GPU 证据时不允许跳过"
            );
            return;
        };
        // A：真实导入路径的 300 骨三角形（v0 = [255, 256] 各 0.5、
        // v1 = 四个全越界、v2 = 只在界内；动画平移节点 255）。
        let path = std::env::temp_dir().join(format!(
            "xgame-oversized-skin-gpu-{}.gltf",
            std::process::id()
        ));
        std::fs::write(&path, crate::gltf_loader::tests::oversized_skin_gltf()).unwrap();
        let mut a = GltfMesh::from_file_with(
            &shot.device,
            &shot.queue,
            &path,
            xgame_core::ImportSettings::default(),
        )
        .expect("合成超限骨架应能加载");
        std::fs::remove_file(&path).unwrap();
        a.bounds = Bounds {
            min: Vec3::splat(-2.0),
            max: Vec3::splat(2.0),
        };
        // B：2 骨玩具，骨 0 平移 (0,0,1)。若 A 的越界下标泄漏，读到的正是
        // B 的骨 0 —— B 一动 A 的顶点就跟。
        let b = skinned_cube(
            &shot,
            "oversized-b",
            2,
            |_| ([0.0; 4], [1.0, 0.0, 0.0, 0.0]),
            &[(0, Vec3::new(0.0, 0.0, 1.0))],
        );
        shot.inject_model("oversized-a", a);
        shot.inject_model("oversized-b", b);
        let mut scene = Scene::new("oversized");
        scene.ambient = [0.22; 3];
        scene.lights = vec![Light::directional("sun", [0.0, -1.0, 0.0], [1.0; 3], 1.6)];
        scene.lights[0].casts_shadow = false;
        for (name, key, x) in [
            ("actor-a", "oversized-a", -1.5),
            ("actor-b", "oversized-b", 1.5),
        ] {
            let mut actor = Entity::new(name);
            actor.model = key.into();
            actor.transform.position = Vec3::new(x, 1.5, 0.0);
            actor.animator = Some(xgame_core::Animator {
                clip: "move".into(),
                looping: false,
                ..Default::default()
            });
            scene.add_entity(actor);
        }
        let camera = ShotCamera::looking_at(
            Vec3::new(0.0, 5.0, 9.0),
            Vec3::new(0.0, 1.0, 0.0),
            DEFAULT_FOV_Y,
        );
        let render = |shot: &mut ScreenshotRenderer, scene: &Scene, anim: &AnimationDriver| {
            shot.render_scene(
                scene,
                anim,
                Some(camera),
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .unwrap();
            shot.read_rgba().unwrap()
        };

        let mut previous: Option<Vec<u8>> = None;
        for time in [0.0, 0.5, 1.0] {
            let mut anim = AnimationDriver::new();
            anim.tick(time, &scene.entities);
            let actual = render(&mut shot, &scene, &anim);
            assert_eq!(
                shot.frame_stats().opaque_instances,
                2,
                "两个角色必须参与绘制"
            );
            if let Some(previous) = previous.as_ref() {
                assert!(actual != *previous, "骨 255 的平移必须真的进入画面");
            }
            previous = Some(actual.clone());

            let mut reference = scene.clone();
            // A 的参考：v0/v2 跟骨 255（t 时刻平移 (t,0,0)），v1 冻结在绑定姿态。
            let ref_a = {
                let positions = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
                let data = crate::mesh::MeshData {
                    vertices: positions
                        .iter()
                        .enumerate()
                        .map(|(i, p)| {
                            let moved = if i == 1 {
                                *p
                            } else {
                                [p[0] + time, p[1], p[2]]
                            };
                            crate::gltf_loader::GltfVertex {
                                position: moved,
                                normal: [0.0, 0.0, 1.0],
                                color: [1.0; 3],
                                tex_coord: [0.0; 2],
                                joints: [0.0; 4],
                                weights: [0.0; 4],
                            }
                        })
                        .collect(),
                    indices: vec![0, 1, 2],
                };
                let mut m =
                    GltfMesh::from_mesh_data(&shot.device, &shot.queue, &data, "oversized-a-ref");
                m.bounds = Bounds {
                    min: Vec3::splat(-2.0),
                    max: Vec3::splat(2.0),
                };
                m
            };
            // B 的参考：全跟骨 0（t 时刻平移 (0,0,t)）。
            let ref_b = baked_cube(&shot, "oversized-b-ref", |_| Vec3::new(0.0, 0.0, time));
            shot.inject_model("oversized-a-ref", ref_a);
            shot.inject_model("oversized-b-ref", ref_b);
            reference.entities[0].model = "oversized-a-ref".into();
            reference.entities[0].animator = None;
            reference.entities[1].model = "oversized-b-ref".into();
            reference.entities[1].animator = None;
            let expected = render(&mut shot, &reference, &AnimationDriver::new());
            let different = pixel_differences(&actual, &expected);
            assert_eq!(
                different, 0,
                "超限影响必须清零（冻结 / 重归一），不能读邻居的槽：t={time}"
            );
        }
    }

    /// **真实 glTF 多关节蒙皮**（R03/031 的"真实 glTF"验收）：24 骨狐狸的
    /// Walk 剪辑在三个时刻的 GPU 蒙皮结果，与测试内**独立计算**的 CPU 蒙皮
    /// 参考对照。骨骼矩阵与引擎同源（`compute_bones_composed`，另有单测覆盖），
    /// 蒙皮求和公式在测试里另写一份 —— 对照的是混合公式本身。
    ///
    /// 真实权重是任意小数、骨骼带旋转 / 层级 / 非单位逆绑定，GPU 与 CPU 的
    /// 浮点求值顺序可能差一个 ULP，因此允许 0.5% 以内的边缘像素不同；混合
    /// 权重若真算错（比如只取第一根骨），差异会是整只狐狸级别的。
    #[test]
    fn real_fox_walk_pose_matches_cpu_skinning_reference() {
        let Some(mut shot) = gpu_or_skip("real_fox_walk_pose_matches_cpu_skinning_reference")
        else {
            assert!(
                std::env::var_os("XGAME_GPU_EVIDENCE_DIR").is_none(),
                "请求 GPU 证据时不允许跳过"
            );
            return;
        };
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../projects/SampleScene/Resources/Fox.gltf");
        let mut animated = GltfMesh::from_file_with(
            &shot.device,
            &shot.queue,
            &path,
            xgame_core::ImportSettings::default(),
        )
        .expect("示例狐狸应能加载");
        assert_eq!(animated.skins.len(), 1, "狐狸有一张蒙皮");
        assert_eq!(animated.skins[0].joints.len(), 24, "24 根骨头");
        let nodes = animated.nodes.clone();
        let skin = animated.skins[0].clone();
        let walk = animated.clip("Walk").expect("Walk 剪辑").clone();
        let fox_bounds = animated.bounds;

        // CPU 侧独立读顶点（不经过引擎的网格加载器）；示例狐狸没有 NORMAL
        // 属性，基座法线与引擎同源（缺属性时的平面法线），蒙皮求和另写一份。
        let (doc, buffers, _) = gltf::import(&path).expect("解析 Fox.gltf");
        let prim = doc.meshes().next().unwrap().primitives().next().unwrap();
        let reader = prim.reader(|b| Some(&buffers[b.index()]));
        let positions: Vec<[f32; 3]> = reader.read_positions().unwrap().collect();
        let normals: Vec<[f32; 3]> = reader
            .read_normals()
            .map(|iter| iter.collect())
            .unwrap_or_else(|| {
                let indices: Vec<u32> = reader
                    .read_indices()
                    .map(|iter| iter.into_u32().collect())
                    .unwrap_or_else(|| (0..positions.len() as u32).collect());
                crate::gltf_loader::compute_face_normals(&positions, &indices)
            });
        let uvs: Vec<[f32; 2]> = reader.read_tex_coords(0).unwrap().into_f32().collect();
        let joints: Vec<[u16; 4]> = reader.read_joints(0).unwrap().into_u16().collect();
        let weights: Vec<[f32; 4]> = reader.read_weights(0).unwrap().into_f32().collect();
        let indices: Vec<u32> = reader
            .read_indices()
            .map(|iter| iter.into_u32().collect())
            .unwrap_or_else(|| (0..positions.len() as u32).collect());
        assert_eq!(positions.len(), 1728, "示例狐狸的顶点数");

        // 参考网格要跟真实狐狸同纹理同材质，否则像素没有可比性。
        // `std::mem::swap` 进出交换：每次迭代把真纹理换进参考网格、渲染后
        // 换回来 —— 放循环里只 `replace` 一次会在第一次迭代偷走源纹理。
        let mut reference_load = GltfMesh::from_file_with(
            &shot.device,
            &shot.queue,
            &path,
            xgame_core::ImportSettings::default(),
        )
        .expect("示例狐狸应能加载（第二份）");
        let fox_material = reference_load.primitives[0].material;

        // 保守盒取真实狐狸那份：两条路径同一份，取景 / 剔除不会成为变量。
        animated.bounds = fox_bounds;
        shot.inject_model("fox-anim", animated);
        let mut scene = Scene::new("fox");
        scene.ambient = [0.22; 3];
        scene.lights = vec![Light::directional("sun", [0.0, -1.0, 0.0], [1.0; 3], 1.6)];
        scene.lights[0].casts_shadow = false;
        let mut actor = Entity::new("fox");
        actor.model = "fox-anim".into();
        actor.animator = Some(xgame_core::Animator {
            clip: "Walk".into(),
            looping: true,
            ..Default::default()
        });
        scene.add_entity(actor);

        let render = |shot: &mut ScreenshotRenderer, scene: &Scene, anim: &AnimationDriver| {
            shot.render_scene(
                scene,
                anim,
                None,
                SceneLighting::new(scene.ambient, &scene.lights),
                RenderConfig::default(),
            )
            .unwrap();
            shot.read_rgba().unwrap()
        };

        let total = 320 * 240;
        let mut previous: Option<Vec<u8>> = None;
        for fraction in [0.0f32, 0.35, 0.7] {
            let time = walk.duration * fraction;
            let mut anim = AnimationDriver::new();
            anim.tick(time, &scene.entities);
            let actual = render(&mut shot, &scene, &anim);
            if let Some(previous) = previous.as_ref() {
                let moved = pixel_differences(&actual, previous);
                assert!(
                    moved > total / 20,
                    "Walk 必须真的动起来：t={time} 只有 {moved} 像素变化"
                );
            }
            previous = Some(actual.clone());

            // CPU 蒙皮参考：skin = (Σ wₖ·boneₖ) / Σwₖ，与 shader 同一公式
            // （在测试里另写一份；法线同式，方向由 fragment 归一化收回）。
            let bones =
                crate::animation::compute_bones_composed(&nodes, &skin, (&walk, time), None, &[]);
            let mut verts: Vec<crate::gltf_loader::GltfVertex> =
                Vec::with_capacity(positions.len());
            for i in 0..positions.len() {
                let wsum = weights[i][0] + weights[i][1] + weights[i][2] + weights[i][3];
                let mut blend = Mat4::ZERO;
                for k in 0..4 {
                    let w = weights[i][k];
                    if w != 0.0 {
                        blend += bones[joints[i][k] as usize] * w;
                    }
                }
                blend *= 1.0 / wsum;
                let p = (blend * Vec3::from(positions[i]).extend(1.0)).truncate();
                // 法线**不归一**：shader 把混合后的原长法线交给插值、fragment
                // 才 normalize —— 提前归一会让三角形内部的插值方向不同。
                let n = (blend * Vec3::from(normals[i]).extend(0.0)).truncate();
                verts.push(crate::gltf_loader::GltfVertex {
                    position: p.to_array(),
                    normal: n.to_array(),
                    color: [1.0; 3],
                    tex_coord: uvs[i],
                    joints: [0.0; 4],
                    weights: [0.0; 4],
                });
            }
            let baked = crate::mesh::MeshData {
                vertices: verts,
                indices: indices.clone(),
            };
            let mut reference =
                GltfMesh::from_mesh_data(&shot.device, &shot.queue, &baked, "fox-reference");
            reference.bounds = fox_bounds;
            // swap 真纹理进来，渲染后 swap 回去（下一轮还要用）。
            std::mem::swap(
                &mut reference.primitives[0].texture,
                &mut reference_load.primitives[0].texture,
            );
            reference.primitives[0].material = fox_material;
            shot.inject_model("fox-reference", reference);
            let mut reference_scene = scene.clone();
            reference_scene.entities[0].model = "fox-reference".into();
            reference_scene.entities[0].animator = None;
            let expected = render(&mut shot, &reference_scene, &AnimationDriver::new());
            // 把真纹理换回 reference_load，供下一轮迭代使用。
            let mut reference_mesh = shot.model_cache.remove("fox-reference").unwrap();
            std::mem::swap(
                &mut reference_mesh.primitives[0].texture,
                &mut reference_load.primitives[0].texture,
            );
            let (different, max_delta, over_two) = pixel_diff_stats(&actual, &expected);
            assert!(
                over_two * 200 <= total,
                "真实狐狸蒙皮与 CPU 参考差异过大：exact={different} max_delta={max_delta} over2={over_two} / {total}（t={time}）"
            );
        }
    }
}