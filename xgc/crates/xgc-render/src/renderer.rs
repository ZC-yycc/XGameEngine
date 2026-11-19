//! 帧渲染入口：DrawList → 线性 HDR 场景 → 可选光晕 → 统一 tone-map → 可选 FXAA → 显示空间 UI。

use std::collections::HashMap;

use super::assets::MeshCache;
use super::bloom::{BloomSettings, BloomStage};
use super::camera::{camera_view_projection_with_eye, invert, Mat4, IDENTITY};
use super::draw::{build_draw_list, DrawList};
use super::exposure::{sanitize_exposure, EXPOSURE_DEFAULT};
use super::frame_color::compose_colors;
use super::fxaa::{FxaaSettings, FxaaStage};
use super::gpu::{GpuContext, GpuError};
use super::gpu_mesh::GpuMeshCache;
use super::image::ImageData;
use super::instanced::{FrameLighting, InstancedRequest};
use super::pipeline::{RenderCache, TrianglePipeline};
use super::texture::TexturePipeline;
use xgc_world::World;

use super::resources::{ResourceDesc, ResourceUsage};
use super::shadow_settings::ShadowSettings;
use super::skinned::{supports_gpu_skin, GpuSkinMeshCache, SkinnedPipeline};
use super::sky::{sky_irradiance, SkySettings};
mod hdr;
mod shadow_prep;
mod ui;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub draws: usize,
    pub draw_calls: usize,
    pub ui_draws: usize,
    /// 逻辑渲染阶段数；不等于底层 wgpu RenderPass 个数。
    pub passes: usize,
    /// 本帧为蒙皮 uniform 竞技场（palette / model）新建的 GPU 缓冲数：同尺寸下恒 0。
    ///
    /// 只统主场景那两张竞技场；阴影 pass 侧另见 [`Self::shadow_skin_uniform_buffers`]。
    pub skin_uniform_buffers: usize,
    /// 本帧为阴影 pass 的蒙皮竞技场（palette / model）新建的 GPU 缓冲数：同尺寸下恒 0。
    pub shadow_skin_uniform_buffers: usize,
}

struct DepthTarget {
    width: u32,
    height: u32,
    view: wgpu::TextureView,
}

pub struct Renderer {
    cache: RenderCache,
    meshes: MeshCache,
    gpu_meshes: GpuMeshCache,
    skin_meshes: GpuSkinMeshCache,
    skinned: Option<(wgpu::TextureFormat, SkinnedPipeline)>,
    sky_pipeline: Option<(wgpu::TextureFormat, super::sky_draw::SkyPipeline)>,
    shadow: Option<super::shadow::ShadowTarget>,
    /// 基色贴图绑定：1×1 白纹理兜底，随第一次渲染惰性创建。
    textures: Option<super::texture_bind::TextureBinding>,
    /// 真字体图集（CPU）与它的 GPU 贴图 / 绘制管线（UI 文本用）。
    font: Option<super::font::FontAtlas>,
    font_texture: Option<(u64, wgpu::Texture)>,
    text_pipeline: Option<(wgpu::TextureFormat, super::text_draw::TextPipeline)>,
    /// 本帧要画的粒子（模拟层在帧循环里算好，渲染只消费四边形）。
    particles: Vec<xgc_world::ParticleQuad>,
    particle_pipeline: Option<(wgpu::TextureFormat, super::particles::ParticlePipeline)>,
    hdr: Option<hdr::HdrStage>,
    /// 画面设置里的曝光（`render.exposure`，缺省 1.0）：曲线前的线性光乘子。
    exposure: f32,
    /// 画面设置里的光晕节：开着时场景在 tone-map 前多跑一遍三级光晕链。
    bloom: BloomSettings,
    bloom_stage: Option<BloomStage>,
    /// 画面设置里的 FXAA 节：开着时 tone-map 先写 LDR scratch，再由 FXAA 平滑到显示目标。
    fxaa: FxaaSettings,
    fxaa_stage: Option<FxaaStage>,
    depth: Option<DepthTarget>,
    images: HashMap<String, Option<ImageData>>,
    ui_overlay: Option<(wgpu::TextureFormat, TrianglePipeline)>,
    ui_pipeline: Option<(wgpu::TextureFormat, TexturePipeline)>,
    ui_textures: HashMap<String, Option<wgpu::Texture>>,
    clear_color: wgpu::Color,
    /// 画面设置里的天空节：开着时环境照明多一份天空漫反射 IBL。
    sky: SkySettings,
    /// 画面设置里的阴影画质（软阴影 / 级间混合）：默认全 0，走老路径逐位不变。
    shadow_settings: ShadowSettings,
}

fn scissor_rect(clip: [f32; 4], viewport: [f32; 2]) -> Option<[u32; 4]> {
    let x = clip[0].max(0.0).min(viewport[0]) as u32;
    let y = clip[1].max(0.0).min(viewport[1]) as u32;
    let right = (clip[0] + clip[2]).max(0.0).min(viewport[0]) as u32;
    let bottom = (clip[1] + clip[3]).max(0.0).min(viewport[1]) as u32;
    if right <= x || bottom <= y {
        None
    } else {
        Some([x, y, right - x, bottom - y])
    }
}

impl Renderer {
    pub fn new(meshes: MeshCache) -> Self {
        Self {
            cache: RenderCache::new(),
            meshes,
            gpu_meshes: GpuMeshCache::new(),
            skin_meshes: GpuSkinMeshCache::new(),
            skinned: None,
            sky_pipeline: None,
            shadow: None,
            textures: None,
            font: None,
            font_texture: None,
            text_pipeline: None,
            particles: Vec::new(),
            particle_pipeline: None,
            hdr: None,
            exposure: EXPOSURE_DEFAULT,
            bloom: BloomSettings::default(),
            bloom_stage: None,
            fxaa: FxaaSettings::default(),
            fxaa_stage: None,
            depth: None,
            images: HashMap::new(),
            ui_overlay: None,
            ui_pipeline: None,
            ui_textures: HashMap::new(),
            clear_color: wgpu::Color::BLACK,
            sky: SkySettings::default(),
            shadow_settings: ShadowSettings::default(),
        }
    }

    /// 设置天空（等价于旧 `project.ron` 的 `render.sky`）。
    pub fn set_sky(&mut self, sky: SkySettings) {
        self.sky = sky.sanitized();
    }

    /// 设置曝光（等价于旧 `project.ron` 的 `render.exposure`）。
    ///
    /// 曝光是曲线之前的线性光乘子，必须与光晕亮部提取用同一个值：0 是
    /// "全黑画面"而不是关闭态，非正 / 非有限值由 [`sanitize_exposure`] 落回
    /// 1.0。缺省 1.0 时帧输出逐字节不变。
    pub fn set_exposure(&mut self, exposure: f32) {
        self.exposure = sanitize_exposure(exposure);
    }

    /// 设置光晕（等价于旧 `project.ron` 的 `render.bloom`）。
    ///
    /// 旧端把光晕放在曝光后、ACES 前：过完曲线的高光没有余量，显示端再叠加
    /// 只会被裁成硬边。`intensity <= 0` 是关闭态，此时整条链不跑也不会被采样。
    pub fn set_bloom(&mut self, bloom: BloomSettings) {
        self.bloom = bloom.sanitized();
    }

    /// 设置 FXAA（等价于旧 `project.ron` 的 `render.fxaa`）。
    ///
    /// 位置与旧端一致：tone-map 之后、UI 之前 —— 抗锯齿只收拾场景边缘，
    /// HUD 文字保持锐利。默认关，关着时整段跳过、帧输出逐字节不变。
    pub fn set_fxaa(&mut self, fxaa: FxaaSettings) {
        self.fxaa = fxaa;
    }

    /// 设置方向光阴影的画质（等价于旧 `project.ron` 的 `render.shadow`）。
    ///
    /// 软阴影（PCSS）与级间混合都是可选增强：两个 0 时着色器走 3×3 PCF + 硬级间
    /// 切换的老路径，帧输出逐字节不变；非有限 / 负值由 [`ShadowSettings::sanitized`]
    /// 落回 0。
    pub fn set_shadow_settings(&mut self, settings: ShadowSettings) {
        self.shadow_settings = settings.sanitized();
    }

    /// 清屏色以线性值写入 HDR 场景，和几何 / 天空 / 粒子一起在帧末 tone-map。
    pub fn set_clear_color(&mut self, color: [f64; 4]) {
        self.clear_color = wgpu::Color {
            r: color[0],
            g: color[1],
            b: color[2],
            a: color[3],
        };
    }

    pub fn meshes(&self) -> &MeshCache {
        &self.meshes
    }
    pub fn gpu_mesh_count(&self) -> usize {
        self.gpu_meshes.len()
    }

    /// 已上传的 GPU 蒙皮网格数；测试 / 诊断用来确认没有静默退回 CPU 形变。
    pub fn gpu_skin_mesh_count(&self) -> usize {
        self.skin_meshes.len()
    }

    fn ensure_depth(
        &mut self,
        context: &GpuContext,
        width: u32,
        height: u32,
    ) -> Result<(), GpuError> {
        if self
            .depth
            .as_ref()
            .is_some_and(|depth| depth.width == width && depth.height == height)
        {
            return Ok(());
        }
        let desc = ResourceDesc::texture2d(
            "xgc-depth",
            "depth32float",
            width.max(1),
            height.max(1),
            vec![ResourceUsage::DepthAttachment],
        );
        let resource = context.create_resource(&desc)?;
        let texture = resource
            .into_texture()
            .ok_or_else(|| GpuError::Pipeline("depth 资源不是纹理".to_string()))?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.depth = Some(DepthTarget {
            width,
            height,
            view,
        });
        Ok(())
    }

    /// 挂上本帧的粒子四边形（由模拟层每帧产出；空表示不画）。
    pub fn set_particles(&mut self, quads: Vec<xgc_world::ParticleQuad>) {
        self.particles = quads;
    }

    /// 渲染一帧：抽 DrawList、解析模型、按实体顺序 draw 到目标纹理。
    pub fn render(
        &mut self,
        context: &GpuContext,
        target: &wgpu::Texture,
        world: &World,
    ) -> Result<RenderStats, GpuError> {
        let mut stats = self.render_selected(context, target, world, None)?;
        stats.ui_draws = self.render_ui(context, target, world)?;
        stats.passes += usize::from(stats.ui_draws > 0);
        Ok(stats)
    }

    /// 截图取景：与旧引擎 `--screenshot` 同一条相机优先级——有场景相机就用场景相机，
    /// 缺相机才退到顺光自动取景（包围盒 + 场景主方向光推视轴）。
    pub fn render_shot(
        &mut self,
        context: &GpuContext,
        target: &wgpu::Texture,
        world: &World,
    ) -> Result<RenderStats, GpuError> {
        let (width, height) = (target.width(), target.height());
        let (view_proj, eye) = match camera_view_projection_with_eye(world, width, height) {
            Some(camera) => camera,
            None => {
                let frame = super::shot::frame_scene(world, &mut self.meshes, width, height)
                    .map_err(|err| GpuError::Mesh(err.to_string()))?;
                (frame.view_projection(width, height), frame.camera.eye)
            }
        };
        let lighting = self.frame_lighting(world, eye);
        let list = self.prepared_list(world, None)?;
        let scene = self.hdr_scene_texture(context, width, height);
        let mut stats = self.render_list(context, &scene, world, list, view_proj, lighting)?;
        self.present_scene(context, target, &scene, &mut stats);
        stats.ui_draws = self.render_ui(context, target, world)?;
        stats.passes += usize::from(stats.ui_draws > 0);
        Ok(stats)
    }

    /// 带选择高亮：选中实体的颜色替换为强调色，用于编辑器视口。
    pub fn render_selected(
        &mut self,
        context: &GpuContext,
        target: &wgpu::Texture,
        world: &World,
        selected: Option<xgc_base::EntityId>,
    ) -> Result<RenderStats, GpuError> {
        let width = target.width();
        let height = target.height();
        let list = self.prepared_list(world, selected)?;
        let (view_proj, eye) =
            camera_view_projection_with_eye(world, width, height).unwrap_or((IDENTITY, [0.0; 3]));
        let lighting = self.frame_lighting(world, eye);
        let scene = self.hdr_scene_texture(context, width, height);
        let mut stats = self.render_list(context, &scene, world, list, view_proj, lighting)?;
        self.present_scene(context, target, &scene, &mut stats);
        Ok(stats)
    }

    /// 帧光照 = 场景灯 + 天空漫反射 IBL（开着才算，太阳取场景主方向光）。
    fn frame_lighting(&self, world: &World, eye: [f32; 3]) -> FrameLighting {
        let environment = world.environment();
        let sun = environment.sun().map(|light| {
            let [x, y, z] = light.normalized_direction();
            ([-x, -y, -z], light.color)
        });
        FrameLighting::from_environment(environment, eye).with_sky(sky_irradiance(&self.sky, sun))
    }

    /// 抽 DrawList 并合成最终颜色：模型材质因子 × 实体材质在前，选择高亮在后。
    fn prepared_list(
        &mut self,
        world: &World,
        selected: Option<xgc_base::EntityId>,
    ) -> Result<DrawList, GpuError> {
        let mut list = build_draw_list(world);
        compose_colors(&mut self.meshes, &mut list, selected)
            .map_err(|err| GpuError::Mesh(err.to_string()))?;
        Ok(list)
    }

    fn draw_particles(
        &mut self,
        context: &GpuContext,
        target: &wgpu::Texture,
        view_proj: Mat4,
        camera_pos: [f32; 3],
        clear_target: bool,
    ) -> Result<usize, GpuError> {
        if self.particles.is_empty() {
            return Ok(0);
        }
        self.ensure_depth(context, target.width(), target.height())?;
        let format = target.format();
        if self.particle_pipeline.as_ref().map(|(current, _)| *current) != Some(format) {
            self.particle_pipeline = Some((
                format,
                super::particles::ParticlePipeline::new(context, format),
            ));
        }
        let depth = &self.depth.as_ref().unwrap().view;
        let Some((_, pipeline)) = self.particle_pipeline.as_ref() else {
            return Ok(0);
        };
        let quads = std::mem::take(&mut self.particles);
        let clear = clear_target.then_some(self.clear_color);
        let request = super::particles::ParticleDrawRequest {
            target,
            depth,
            view_proj,
            camera_pos,
            quads: &quads,
            clear,
        };
        let drawn = pipeline
            .draw(context, &request)
            .map_err(GpuError::Pipeline)?;
        Ok(drawn)
    }

    /// 天空盒：整屏一张三角形，深度 `LessEqual` + 不写深度，被几何挡住的像素不画。
    ///
    /// 单独一个 pass（`LoadOp::Load`）而不是塞进实例化 pass：实例化 pass 自己负责
    /// 清屏与深度清除，天空必须排在所有不透明几何之后。
    fn draw_sky(
        &self,
        context: &GpuContext,
        target: &wgpu::Texture,
        view_proj: Mat4,
        lighting: &FrameLighting,
        clear_depth: bool,
    ) {
        let Some((_, sky_pipeline)) = self.sky_pipeline.as_ref() else {
            return;
        };
        let depth = &self.depth.as_ref().unwrap().view;
        let sun = (lighting.sun_intensity > 0.0).then(|| {
            let [x, y, z] = lighting.sun_direction;
            ([-x, -y, -z], lighting.sun_color)
        });
        let uniform = super::sky::SkyUniform::build(&self.sky, sun);
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xgc-sky-encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-sky-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: if clear_depth {
                            wgpu::LoadOp::Clear(1.0)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            sky_pipeline.draw(
                context,
                &mut pass,
                invert(view_proj).unwrap_or(IDENTITY),
                lighting.eye,
                &uniform,
            );
        }
        context.queue().submit(Some(encoder.finish()));
    }

    fn render_list(
        &mut self,
        context: &GpuContext,
        target: &wgpu::Texture,
        world: &World,
        list: DrawList,
        view_proj: Mat4,
        mut lighting: FrameLighting,
    ) -> Result<RenderStats, GpuError> {
        self.ensure_depth(context, target.width(), target.height())?;
        // 天空管线要在拿 `self.depth` 的不可变借用之前建好（之后只剩读）。
        if self.sky.enabled {
            let format = target.format();
            if self.sky_pipeline.as_ref().map(|(current, _)| *current) != Some(format) {
                self.sky_pipeline = Some((format, context.create_sky_pipeline(format)));
            }
        }
        if self.shadow.is_none() {
            self.shadow = Some(super::shadow::ShadowTarget::new(context));
        }
        if self.textures.is_none() {
            self.textures = Some(super::texture_bind::TextureBinding::new(context));
        }
        // 阴影投射体 = 全部 draw：旧端蒙皮网格与静态网格一样进阴影批次。
        let (shadow_matrix, shadow_skin_uniform_buffers) = self.prepare_shadow(
            context,
            world,
            &list.items,
            view_proj,
            lighting.eye,
            self.shadow_settings.cascade_blend,
        )?;
        let textures = self.textures.as_ref().unwrap();
        lighting = lighting.with_shadow(shadow_matrix).with_shadow_quality(
            self.shadow_settings.softness,
            self.shadow_settings.cascade_blend,
        );
        let (shadow_view, shadow_sampler) = self
            .shadow
            .as_ref()
            .map(|target| target.sample_source(shadow_matrix.is_some()))
            .unwrap();
        let depth = &self.depth.as_ref().unwrap().view;
        let draws = list.len();
        let mut cpu_items = Vec::with_capacity(list.items.len());
        let mut gpu_items = Vec::new();
        for item in list.items {
            let gpu_skin = if let Some(pose) = item.pose.as_deref() {
                // 设备 binding 上限装不下 16 KiB palette 时也必须退回 CPU。
                context.supports_full_skin_palette()
                    && self
                        .meshes
                        .get(&item.model)
                        .map_err(|err| GpuError::Mesh(err.to_string()))?
                        .is_some_and(|mesh| supports_gpu_skin(mesh, pose))
            } else {
                false
            };
            if gpu_skin {
                gpu_items.push(item);
            } else {
                cpu_items.push(item);
            }
        }
        let cpu_list = super::draw::DrawList { items: cpu_items };
        let mut request = InstancedRequest {
            texture: target,
            list: &cpu_list,
            meshes: &mut self.meshes,
            gpu_meshes: &mut self.gpu_meshes,
            view_proj,
            lighting,
            shadow_view,
            shadow_sampler,
            depth,
            clear_color: self.clear_color,
            textures,
        };
        let mut draw_calls = self.cache.draw_list_instanced(context, &mut request)?;
        let mut skin_uniform_buffers = 0usize;
        if !gpu_items.is_empty() {
            let format = target.format();
            if self
                .skinned
                .as_ref()
                .is_none_or(|(current, _)| *current != format)
            {
                self.skinned = Some((
                    format,
                    context.create_skinned_pipeline(format, true, textures.layout()),
                ));
            }
            let pipeline = &mut self.skinned.as_mut().unwrap().1;
            // 第一遍：上传本条需要的蒙皮网格并算好 palette（上传会可变借用缓存）。
            let mut resolved: Vec<(usize, Vec<u8>)> = Vec::with_capacity(gpu_items.len());
            for (index, item) in gpu_items.iter().enumerate() {
                let Some(mesh) = self
                    .meshes
                    .get(&item.model)
                    .map_err(|err| GpuError::Mesh(err.to_string()))?
                else {
                    continue;
                };
                if mesh.positions.is_empty() {
                    continue;
                }
                self.skin_meshes
                    .upload(context, &item.model, mesh, textures);
                let inverse_bind = mesh
                    .skin
                    .as_ref()
                    .map(|skin| skin.inverse_bind.as_slice())
                    .unwrap_or(&[]);
                let palette = super::skinned::palette_bytes(
                    item.pose.as_deref().unwrap_or(&[]),
                    inverse_bind,
                );
                resolved.push((index, palette));
            }
            // 第二遍：只读缓存组出整批 draw，一批共用一个 frame binding / 一次提交。
            let mut batch = Vec::with_capacity(resolved.len());
            for (index, palette) in &resolved {
                let item = &gpu_items[*index];
                let Some(gpu_mesh) = self.skin_meshes.get(&item.model) else {
                    continue;
                };
                batch.push(super::skinned::SkinnedDraw {
                    mesh: gpu_mesh,
                    palette,
                    model: item.transform,
                    color: item.color,
                    metallic: item.metallic,
                    roughness: item.roughness,
                });
            }
            if !batch.is_empty() {
                let frame_bind = super::instanced::create_frame_bind(
                    context,
                    pipeline.frame_layout(),
                    view_proj,
                    &lighting,
                    shadow_view,
                    shadow_sampler,
                );
                let skinned = pipeline.draw_batch(
                    context,
                    &super::skinned::SkinnedBatchRequest {
                        texture: target,
                        depth,
                        clear: draw_calls == 0,
                        clear_color: self.clear_color,
                        frame_bind: &frame_bind,
                        draws: &batch,
                    },
                )?;
                draw_calls += skinned.draws;
                skin_uniform_buffers += skinned.uniform_buffers;
            }
        }
        if self.sky.enabled {
            // 没有场景 draw 时实例化 pass 提前返回、没清深度：天空 pass 自己清成 1.0，否则只能 Load。
            self.draw_sky(context, target, view_proj, &lighting, draws == 0);
        }
        // 粒子画在几何 / 天空之后、UI 之前：要被几何遮住，又要参与色调映射。
        draw_calls += self.draw_particles(
            context,
            target,
            view_proj,
            lighting.eye,
            draws == 0 && !self.sky.enabled,
        )?;
        Ok(RenderStats {
            draws,
            draw_calls,
            ui_draws: 0,
            // HDR 场景即使没有 draw 也会清色；随后帧入口再计一次 tone-map。
            passes: 1,
            skin_uniform_buffers,
            shadow_skin_uniform_buffers,
        })
    }
}
