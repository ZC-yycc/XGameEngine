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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use glam::{Mat4, Vec3};

use xgame_render::gltf_loader::{Bounds, GltfMesh};
use xgame_render::scene_pipeline::{SceneLighting, ScenePipeline, MAX_LIGHTS};
use xgame_core::{Entity, EntityId, Scene};

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
    /// 最终交给 egui 的**单采样**颜色纹理。
    render_view: Option<wgpu::TextureView>,
    /// 开了 MSAA 时的多采样颜色目标；渲染写它，再 resolve 到 `render_view`。
    msaa_color_view: Option<wgpu::TextureView>,
    /// 深度附件。采样数必须与颜色附件一致（MSAA 时也是多采样）。
    depth_view: Option<wgpu::TextureView>,
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

        let pipeline = ScenePipeline::new(&device, surface_format, sample_count);

        Self {
            device,
            queue,
            egui_renderer,
            surface_format,
            pipeline,
            project_root,
            viewport_size: (0, 0),
            render_view: None,
            msaa_color_view: None,
            depth_view: None,
            sample_count: sample_count.max(1),
            _stale_textures: Vec::new(),
            texture_id: None,
            last_signature: None,
            model_cache: HashMap::new(),
        }
    }

    // -- Viewport management --

    pub fn set_viewport_size(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || self.viewport_size == (width, height) {
            return;
        }
        self.viewport_size = (width, height);
        self.recreate_targets();
    }

    fn recreate_targets(&mut self) {
        let (w, h) = self.viewport_size;

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
        self.msaa_color_view = None;
        self.depth_view = None;
        // 尺寸变了，之前那张纹理的内容已经作废，下一帧无论如何都要重画。
        self.last_signature = None;

        let size = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };

        let color = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("editor_scene_color"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.surface_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());

        // 开了 MSAA 就再要一张多采样颜色目标：渲染写它、resolve 到上面的单采样图。
        // 另外把旧纹理塞进 `_stale_textures` 只是为了让它们别在飞行中的提交还没
        // 完成时就被销毁（egui 那边还引用着上一帧的 texture id）。
        let msaa_color = (self.sample_count > 1).then(|| {
            let t = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("editor_scene_color_msaa"),
                size,
                mip_level_count: 1,
                sample_count: self.sample_count,
                dimension: wgpu::TextureDimension::D2,
                format: self.surface_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            t.create_view(&wgpu::TextureViewDescriptor::default())
        });

        // 深度附件的采样数**必须**与颜色附件一致，否则渲染通道校验失败。
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("editor_scene_depth"),
            size,
            mip_level_count: 1,
            sample_count: self.sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

        // Register with egui-wgpu（注册的必须是**单采样**那张 —— egui 只认它）
        let tid = self
            .egui_renderer
            .write()
            .register_native_texture(&self.device, &color_view, wgpu::FilterMode::Linear);

        self.render_view = Some(color_view);
        self.msaa_color_view = msaa_color;
        self.depth_view = Some(depth_view);
        self.texture_id = Some(tid);
        self._stale_textures.push(color);
        self._stale_textures.push(depth);
        if self._stale_textures.len() > 24 {
            self._stale_textures.drain(..12);
        }
    }

    pub fn texture_id(&self) -> Option<egui::TextureId> {
        self.texture_id
    }

    /// 某个模型标识对应的**本地**包围盒（已加载过才有）。
    ///
    /// 给视口拾取用：拾取要按实体的真实几何范围判定，而"包围球半径 = |scale|/2"
    /// 对拉伸过的物体会严重高估 —— 一块 16×0.5×16 的地面半径会是 16，连相机都
    /// 落在球里，于是点哪儿都先选中它。
    ///
    /// 返回 `Option` 而不是兜底：调用方（`pick_entity`）知道"内置图元就是单位盒"
    /// 这个语义，让它自己决定怎么兜底，比在这里偷偷塞一个默认值清楚。
    pub fn mesh_bounds(&self, model: &str) -> Option<Bounds> {
        self.model_cache.get(model).map(|m| m.bounds)
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
    /// 光源，以及每个实体的模型标识、**局部变换**与**父引用**。选中态、gizmo、
    /// 网格开关、物理调试线都不进来 —— 它们画在 egui 叠加层上，与这张离屏纹理无关。
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
    fn scene_signature(
        viewport: (u32, u32),
        view_proj: &Mat4,
        camera_pos: &Vec3,
        lighting: &SceneLighting<'_>,
        entities: &[Entity],
    ) -> u64 {
        let mut hash = 0;

        hash = mix_bits(hash, viewport.0 as u64);
        hash = mix_bits(hash, viewport.1 as u64);
        hash = mix_f32s(hash, &view_proj.to_cols_array());
        hash = mix_f32s(hash, &[camera_pos.x, camera_pos.y, camera_pos.z]);
        hash = mix_f32s(hash, &lighting.ambient);

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
        }

        hash
    }

    // -- Rendering --

    /// 渲染一帧离屏场景。
    ///
    /// 返回 `true` 表示这一帧确实重画了；返回 `false` 表示输入指纹与上一帧逐位相同，
    /// **整段渲染被跳过**了 —— 此时上一帧的纹理仍然有效，调用方照常拿
    /// [`texture_id`](Self::texture_id) 去画就行，画面上不会有任何区别。
    pub fn render(
        &mut self,
        scene: &Scene,
        view_proj: &Mat4,
        camera_pos: &Vec3,
        lighting: SceneLighting<'_>,
    ) -> bool {
        let (w, h) = self.viewport_size;
        if w == 0 || h == 0 {
            return false;
        }

        let entities = &scene.entities;

        // 先算指纹：**刻意排在解算世界矩阵之前**。
        //
        // 指纹只需要局部变换 + 父引用（等价于世界矩阵，见 `scene_signature` 的说明），
        // 所以静态帧可以在这里直接返回 —— 连 `Scene::world_matrices()` 那几条 O(n)
        // 的分配与矩阵连乘都省掉。编辑器绝大多数帧都是这种"什么都没动"的帧。
        let signature = Self::scene_signature(
            self.viewport_size,
            view_proj,
            camera_pos,
            &lighting,
            entities,
        );
        if self.last_signature == Some(signature) {
            return false;
        }

        // 层级在 core 里解算：一次算出所有实体的世界矩阵，供指纹与管线共用。
        let world_matrices = scene.world_matrices();

        // Phase 1: ensure all models are loaded (mutable borrow on model_cache)
        //
        // 放在指纹判断**之后**是有意的：模型标识本身就进了指纹，所以新加的实体、
        // 改过路径的实体一定会走到这里，不会漏加载；而指纹没变的帧连一次表查找都
        // 不做，更不会去反复读一个坏路径的磁盘文件 —— 那正是"静态界面也卡"的经典成因。
        for entity in entities {
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
                        self.model_cache.insert(entity.model.clone(), m);
                    }
                    Err(e) => log::warn!("Failed to load model '{}': {:#}", entity.model, e),
                }
            }
        }

        // Phase 2: render via shared pipeline
        let Some(ref color_view) = self.render_view else {
            return false;
        };
        let Some(ref depth_view) = self.depth_view else {
            return false;
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
            color_view,
            self.msaa_color_view.as_ref(),
            depth_view,
            entities,
            &world_matrices,
            &self.model_cache,
            view_proj,
            camera_pos,
            lighting,
        );

        self.queue.submit(std::iter::once(encoder.finish()));

        // 只有真画了才记账 —— 半路 return 的那些分支都没产出可用图像。
        self.last_signature = Some(signature);
        true
    }
}

/// 把一个 64 位值搅进哈希（splitmix64 的收尾混合）。
///
/// 自己写而不引第三方哈希库：这里只要"稳定 + 雪崩"，不需要抗碰撞，多一个依赖不划算。
fn mix_bits(hash: u64, value: u64) -> u64 {
    let mut x = hash ^ value.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    x
}

/// 把一串 f32 按**位模式**搅进哈希。
fn mix_f32s(hash: u64, values: &[f32]) -> u64 {
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
        )
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
        );
        assert_ne!(reference, moved_camera, "相机矩阵没进指纹");

        // 7. 视口尺寸
        let resized = SceneRenderer::scene_signature(
            (1024, 600),
            &Mat4::IDENTITY,
            &Vec3::new(5.0, 5.0, 5.0),
            &SceneLighting::new(ambient, &lights),
            &base,
        );
        assert_ne!(reference, resized, "视口尺寸没进指纹");
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
