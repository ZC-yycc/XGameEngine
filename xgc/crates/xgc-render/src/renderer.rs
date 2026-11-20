//! 帧渲染入口：DrawList → 线性 HDR 场景 → 可选光晕 → 统一 tone-map → 可选 FXAA → 显示空间 UI。

use std::collections::HashMap;

use super::assets::MeshCache;
use super::bloom::{BloomSettings, BloomStage};
use super::camera::{camera_view_projection_with_eye, Mat4, IDENTITY};
use super::draw::{build_draw_list, DrawList};
use super::exposure::EXPOSURE_DEFAULT;
use super::frame_color::compose_colors;
use super::frame_uniform::{create_frame_bind, FrameLighting, FrameTextures};
use super::fxaa::{FxaaSettings, FxaaStage};
use super::gpu::{GpuContext, GpuError};
use super::gpu_mesh::GpuMeshCache;
use super::image::ImageData;
use super::instanced::InstancedRequest;
use super::pipeline::{RenderCache, TrianglePipeline};
use super::texture::TexturePipeline;
use xgc_world::World;

use super::shadow_settings::ShadowSettings;
use super::skinned::{supports_gpu_skin, GpuSkinMeshCache, SkinnedPipeline};
use super::sky::{sky_irradiance, SkySettings};
use super::ssao::SsaoSettings;
use super::taa::TaaSettings;
use scene_target::{DepthTarget, SceneTarget};
mod hdr;
mod scene_target;
mod settings;
mod shadow_prep;
mod sky_pass;
mod ssao_pass;
mod taa_pass;
use taa_pass::CameraMatrices;
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
    /// 点光立方体阴影本帧画了多少次（六面 × 投射体）：0 = 这一帧没做点光阴影。
    pub point_shadow_draw_calls: usize,
    /// 预过滤环境立方体本帧跑的 pass 数：天空设置没变恒 0，首次建 30 级面 + 1 张 BRDF 表。
    pub env_map_passes: usize,
}

pub struct Renderer {
    cache: RenderCache,
    meshes: MeshCache,
    gpu_meshes: GpuMeshCache,
    skin_meshes: GpuSkinMeshCache,
    skinned: Option<(wgpu::TextureFormat, u32, SkinnedPipeline)>,
    sky_pipeline: Option<(wgpu::TextureFormat, u32, super::sky_draw::SkyPipeline)>,
    shadow: Option<super::shadow::ShadowTarget>,
    /// 基色贴图绑定：1×1 白纹理兜底，随第一次渲染惰性创建。
    textures: Option<super::texture_bind::TextureBinding>,
    /// 真字体图集（CPU）与它的 GPU 贴图 / 绘制管线（UI 文本用）。
    font: Option<super::font::FontAtlas>,
    font_texture: Option<(u64, wgpu::Texture)>,
    text_pipeline: Option<(wgpu::TextureFormat, super::text_draw::TextPipeline)>,
    /// 本帧要画的粒子（模拟层在帧循环里算好，渲染只消费四边形）。
    particles: Vec<xgc_world::ParticleQuad>,
    particle_pipeline: Option<(wgpu::TextureFormat, u32, super::particles::ParticlePipeline)>,
    hdr: Option<hdr::HdrStage>,
    /// 显式指定的场景采样数；`None` 跟适配器（`GpuContext::scene_sample_count`）。
    sample_count: Option<u32>,
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
    /// 画面设置里的时间抗锯齿：默认关，关着时不建历史、相机不抖动。
    taa: TaaSettings,
    taa_stage: Option<crate::taa::TaaStage>,
    /// 画面设置里的 SSAO 节：开着时场景色在光晕之前先乘一遍遮蔽。
    ssao: SsaoSettings,
    ssao_stage: Option<crate::ssao::SsaoStage>,
    /// 解析天空的 GPU 预过滤（环境镜面 split-sum 的第一项）与 BRDF 表（第二项）。
    env_map: Option<crate::env_map::EnvMapStage>,
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
            sample_count: None,
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
            taa: TaaSettings::default(),
            taa_stage: None,
            ssao: SsaoSettings::default(),
            ssao_stage: None,
            env_map: None,
        }
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

    /// 场景 pass 的采样数：`None` 跟适配器判定（常见 4×），`Some(1)` 是显式关掉
    /// （测试里的"硬边"前提、以及将来质量档位都要它）。
    pub fn set_particles(&mut self, quads: Vec<xgc_world::ParticleQuad>) {
        self.particles = quads;
    }

    /// 场景 pass 的采样数：`1` = 关 MSAA，其它值取适配器支持的最高档（4×）。
    /// 改档会丢掉深度缓存，管线 / HDR 多采样目标按采样数缓存的键自动重建。
    pub fn set_sample_count(&mut self, count: u32) {
        let count = if count > 1 {
            crate::msaa::PREFERRED_SAMPLE_COUNT
        } else {
            1
        };
        if self.sample_count != Some(count) {
            self.sample_count = Some(count);
            self.depth = None;
        }
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
        let scene_view_proj = self.taa_begin(context, width, height, view_proj);
        let scene = self.hdr_scene_texture(context, width, height);
        let mut stats = self.render_list(
            context,
            &scene,
            world,
            list,
            CameraMatrices {
                scene: scene_view_proj,
                shadow: view_proj,
            },
            lighting,
        )?;
        let resolved = self.taa_resolve(context, &scene, view_proj, eye, &mut stats);
        let shaded = self.ssao_apply(context, &resolved, view_proj, eye, &mut stats);
        self.present_scene(context, target, &shaded, &mut stats);
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
        let scene_view_proj = self.taa_begin(context, width, height, view_proj);
        let scene = self.hdr_scene_texture(context, width, height);
        let mut stats = self.render_list(
            context,
            &scene,
            world,
            list,
            CameraMatrices {
                scene: scene_view_proj,
                shadow: view_proj,
            },
            lighting,
        )?;
        let resolved = self.taa_resolve(context, &scene, view_proj, eye, &mut stats);
        let shaded = self.ssao_apply(context, &resolved, view_proj, eye, &mut stats);
        self.present_scene(context, target, &shaded, &mut stats);
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
        view: &wgpu::TextureView,
        target: SceneTarget,
        view_proj: Mat4,
        camera_pos: [f32; 3],
        clear_target: bool,
    ) -> Result<usize, GpuError> {
        if self.particles.is_empty() {
            return Ok(0);
        }
        if self
            .particle_pipeline
            .as_ref()
            .is_none_or(|(current, count, _)| {
                *current != target.format || *count != target.sample_count
            })
        {
            self.particle_pipeline = Some((
                target.format,
                target.sample_count,
                super::particles::ParticlePipeline::new(
                    context,
                    target.format,
                    target.sample_count,
                ),
            ));
        }
        let depth = &self.depth.as_ref().unwrap().view;
        let Some((_, _, pipeline)) = self.particle_pipeline.as_ref() else {
            return Ok(0);
        };
        let quads = std::mem::take(&mut self.particles);
        let clear = clear_target.then_some(self.clear_color);
        let request = super::particles::ParticleDrawRequest {
            view,
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

    fn render_list(
        &mut self,
        context: &GpuContext,
        target: &wgpu::Texture,
        world: &World,
        list: DrawList,
        cameras: CameraMatrices,
        mut lighting: FrameLighting,
    ) -> Result<RenderStats, GpuError> {
        let CameraMatrices {
            scene: view_proj,
            shadow: shadow_view_proj,
        } = cameras;
        self.ensure_frame_targets(context, target)?;
        let views = self.hdr_scene_views(context, self.scene_sample_count(context));
        let format = target.format();
        let sample_count = views.sample_count;
        let scene_view = &views.view;
        let resolve_view = views.resolve.as_ref();
        // 阴影投射体 = 全部 draw：旧端蒙皮网格与静态网格一样进阴影批次。
        // 拟合用**未抖动**矩阵（TAA 的抖动只叠在相机上，见 `taa_pass.rs`）。
        let prepared = self.prepare_shadow(
            context,
            world,
            &list.items,
            shadow_view_proj,
            lighting.eye,
            self.shadow_settings.cascade_blend,
        )?;
        let textures = self.textures.as_ref().unwrap();
        let shadow_matrix = prepared.cascades;
        let point_shadow = prepared.point;
        lighting = lighting
            .with_shadow(shadow_matrix)
            .with_shadow_quality(
                self.shadow_settings.softness,
                self.shadow_settings.cascade_blend,
            )
            .with_point_shadow(point_shadow);
        let shadow_target = self.shadow.as_ref().unwrap();
        let (shadow_view, shadow_sampler) = shadow_target.sample_source(shadow_matrix.is_some());
        let point_shadow_view = shadow_target.point_shadow_source(point_shadow.is_some());
        // 环境镜面项：解析天空 → GGX 5 级预过滤立方体 + BRDF 积分表（设置不变时零 pass）。
        //
        // 天空关着也建一次（约 1 MiB 纹理 + 两条管线）：帧绑定组的 4 / 5 / 6 槽必须始终
        // 有合法视图；着色端有 `sky_params.x > 0.5` 守卫，关着不采样，所以画面逐字节不变。
        if self.env_map.is_none() {
            self.env_map = Some(crate::env_map::EnvMapStage::new(context));
        }
        let sun = world.environment().sun().map(|light| {
            let [x, y, z] = light.normalized_direction();
            ([-x, -y, -z], light.color)
        });
        let env_map_passes = self
            .env_map
            .as_mut()
            .map(|stage| stage.prepare(context, &self.sky, sun))
            .unwrap_or(0);
        let (env_cube, brdf_lut, env_sampler) = self.env_map.as_ref().unwrap().views();
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
            view: scene_view,
            sample_count,
            format,
            list: &cpu_list,
            meshes: &mut self.meshes,
            gpu_meshes: &mut self.gpu_meshes,
            view_proj,
            lighting,
            shadow_view,
            shadow_sampler,
            point_shadow_view,
            env_cube,
            brdf_lut,
            env_sampler,
            depth,
            clear_color: self.clear_color,
            textures,
        };
        let mut draw_calls = self.cache.draw_list_instanced(context, &mut request)?;
        let mut skin_uniform_buffers = 0usize;
        if !gpu_items.is_empty() {
            if self
                .skinned
                .as_ref()
                .is_none_or(|(current, count, _)| *current != format || *count != sample_count)
            {
                self.skinned = Some((
                    format,
                    sample_count,
                    context.create_skinned_pipeline(format, true, textures.layout(), sample_count),
                ));
            }
            let pipeline = &mut self.skinned.as_mut().unwrap().2;
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
                let frame_bind = create_frame_bind(
                    context,
                    pipeline.frame_layout(),
                    view_proj,
                    &lighting,
                    FrameTextures {
                        shadow: shadow_view,
                        shadow_sampler,
                        point_shadow: point_shadow_view,
                        env_cube,
                        brdf_lut,
                        env_sampler,
                    },
                );
                let skinned = pipeline.draw_batch(
                    context,
                    &super::skinned::SkinnedBatchRequest {
                        view: scene_view,
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
            self.draw_sky(
                context,
                scene_view,
                SceneTarget {
                    format,
                    sample_count,
                },
                view_proj,
                &lighting,
                draws == 0,
            );
        }
        // 粒子画在几何 / 天空之后、UI 之前：要被几何遮住，又要参与色调映射。
        draw_calls += self.draw_particles(
            context,
            scene_view,
            SceneTarget {
                format,
                sample_count,
            },
            view_proj,
            lighting.eye,
            draws == 0 && !self.sky.enabled,
        )?;
        // 场景各 pass 都不带 resolve（否则每个 pass 都多一次全屏拷贝），收尾做一次。
        if let Some(resolve) = resolve_view {
            context.resolve_scene_view(scene_view, resolve);
        }
        Ok(RenderStats {
            draws,
            draw_calls,
            ui_draws: 0,
            // HDR 场景即使没有 draw 也会清色；随后帧入口再计一次 tone-map。
            passes: 1,
            skin_uniform_buffers,
            shadow_skin_uniform_buffers: prepared.skin_uniform_buffers,
            point_shadow_draw_calls: prepared.point_draw_calls,
            env_map_passes,
        })
    }
}
