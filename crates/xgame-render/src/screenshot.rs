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
    let dir = direction.try_normalize().unwrap_or_else(default_shot_direction);
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
    /// 模型标识 → 已上传的网格（与编辑器 / 游戏同一条加载入口）。
    model_cache: HashMap<String, GltfMesh>,
    /// 磁盘模型的解析基准（`builtin://` 用不到）。
    project_root: PathBuf,
}

impl ScreenshotRenderer {
    /// 建 GPU 设备与离屏附件。场景 pass 渲进 `Rgba16Float`（HDR），后处理
    /// pass 做色调映射写进 `Rgba8UnormSrgb`（可读回的那张）；MSAA 采样数
    /// 按 **HDR 格式**的能力选（与编辑器视口同一规则 —— 采样数是格式相关的，
    /// 拿显示格式问出来的答案在 HDR 附件上不一定成立）。
    pub fn new(project_root: &Path, width: u32, height: u32) -> Result<Self> {
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
        let sample_count = supported_sample_count(&adapter, HDR_FORMAT);
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
        let pipeline = ScenePipeline::new(&device, HDR_FORMAT, sample_count);
        let postfx = PostFx::new(&device, format);

        Ok(Self {
            device,
            queue,
            pipeline,
            size: (width, height),
            _ldr: ldr,
            ldr_view,
            hdr,
            postfx,
            model_cache: HashMap::new(),
            project_root: project_root.to_path_buf(),
        })
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// 上一次 [`render_scene`](Self::render_scene) 的绘制统计（口径见
    /// [`ScenePipeline::frame_stats`]）。
    ///
    /// 实例化是否真的合批了只有这个读数看得见：像素测试能守"画得对"，
    /// 守不了"是不是一批画完的"。
    pub fn frame_stats(&self) -> FrameStats {
        self.pipeline.frame_stats()
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
        // 模型懒加载：与编辑器 SceneRenderer 同一条入口，失败只警告
        // （画不出来的实体留空，整张图照常出）。
        for entity in &scene.entities {
            if entity.model.is_empty() || self.model_cache.contains_key(&entity.model) {
                continue;
            }
            match GltfMesh::load(&self.device, &self.queue, &self.project_root, &entity.model) {
                Ok(m) => {
                    self.model_cache.insert(entity.model.clone(), m);
                }
                Err(e) => log::warn!("截图：加载模型 '{}' 失败: {:#}", entity.model, e),
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
        let radius = bounds.map(|b| b.radius()).unwrap_or(EMPTY_SCENE_FALLBACK_RADIUS);
        let (near, far) = clip_planes(&camera, radius);

        let view = Mat4::look_at_rh(camera.eye, camera.target, camera.up);
        let proj = Mat4::perspective_rh(camera.fov_y, self.aspect(), near, far);
        let view_proj = proj * view;

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
            &camera.eye,
            lighting,
            settings.sky,
        );
        // HDR 场景图 → 曝光 +（Bloom）→ 色调映射 → LDR 显示目标。截图器建一次
        // 用一次、目标从不重建 ⇒ 目标代数恒为 0（绑定组缓存只填一次）；
        // Bloom 链按尺寸惰性建，`source_size` 就是渲染尺寸。
        self.postfx.render(
            &self.device,
            &self.queue,
            &mut encoder,
            &self.hdr.color_view,
            0,
            self.size,
            &self.ldr_view,
            settings,
        );
        self.queue.submit(std::iter::once(encoder.finish()));
        Ok(camera)
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
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("创建截图目录失败 ({})", parent.display()))?;
            }
        }
        image::save_buffer_with_format(
            path,
            &rgba,
            width,
            height,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .with_context(|| format!("写 PNG 失败 ({})", path.display()))?;
        Ok((width, height))
    }
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

    use xgame_core::{BloomConfig, Entity, Light, MaterialOverride, SkyConfig, ToneMapMode};

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
        assert!(near > 0.0 && near < dist + radius, "near={near} dist={dist}");
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

    /// 建一个截图渲染器；无 GPU 环境跳过（返回 None）。
    fn gpu_or_skip(name: &str) -> Option<ScreenshotRenderer> {
        match ScreenshotRenderer::new(&std::env::temp_dir(), 320, 240) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("跳过 {name}：{e}");
                None
            }
        }
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
    /// 特意开满 Bloom：链的惰性建（首帧重建纹理与绑定组）与 9 趟 pass 的
    /// 记录顺序都必须确定 —— 关着测只覆盖合成 pass 一条路径。
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

        first
            .render_scene(
                &scene,
                &anim,
                None,
                SceneLighting::new(scene.ambient, &scene.lights),
                bloom_settings(0.7),
            )
            .expect("第一次渲染失败");
        let (w, h) = first.save_png(&path_a).expect("第一次落盘失败");

        // 重建整个渲染器（新设备、新管线、新附件），模拟"两次进程运行"。
        let mut second = ScreenshotRenderer::new(&std::env::temp_dir(), 320, 240)
            .expect("第二次创建渲染器失败");
        second
            .render_scene(
                &scene,
                &anim,
                None,
                SceneLighting::new(scene.ambient, &scene.lights),
                bloom_settings(0.7),
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
        let radius = bounds.map(|b| b.radius()).unwrap_or(EMPTY_SCENE_FALLBACK_RADIUS);
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

        let mut second = ScreenshotRenderer::new(&std::env::temp_dir(), 320, 240)
            .expect("第二次创建渲染器失败");
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
        assert_eq!(
            bytes_a, bytes_b,
            "天空开着时两次截图的 PNG 必须逐字节一致"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
        scene.lights = vec![Light::directional("sun", [0.6, -1.0, -0.75], [1.0; 3], intensity)];
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
        let Some(mut shot) = gpu_or_skip("exposure_scales_highlights_before_the_curve_on_gpu") else {
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
            for (i, p) in [(-1.5, 0.5, -0.5), (1.5, 0.5, -0.5), (-1.5, 0.5, 1.0), (1.5, 0.5, 1.0)]
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

        let camera =
            ShotCamera::looking_at(Vec3::new(0.0, 4.0, 6.0), Vec3::new(0.0, 0.0, 0.2), DEFAULT_FOV_Y);
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
}
