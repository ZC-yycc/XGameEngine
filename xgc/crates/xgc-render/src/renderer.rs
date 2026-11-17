//! 帧渲染入口：DrawList → MeshCache → RenderCache → GPU。

use std::collections::HashMap;

use super::assets::MeshCache;
use super::camera::{camera_view_projection_with_eye, invert, Mat4, IDENTITY};
use super::draw::{build_draw_list, DrawList};
use super::frame_color::compose_colors;
use super::gpu::{GpuContext, GpuError};
use super::gpu_mesh::GpuMeshCache;
use super::image::ImageData;
use super::instanced::{FrameLighting, InstancedRequest};
use super::pipeline::{RenderCache, TrianglePipeline};
use super::texture::TexturePipeline;
use xgc_world::World;

use super::resources::{ResourceDesc, ResourceUsage};
use super::skinned::{GpuSkinMeshCache, SkinnedPipeline, SkinnedRequest};
use super::sky::{sky_irradiance, SkySettings};
mod ui;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub draws: usize,
    pub draw_calls: usize,
    pub ui_draws: usize,
    /// 逻辑渲染阶段数；不等于底层 wgpu RenderPass 个数。
    pub passes: usize,
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
    depth: Option<DepthTarget>,
    images: HashMap<String, Option<ImageData>>,
    ui_overlay: Option<(wgpu::TextureFormat, TrianglePipeline)>,
    ui_pipeline: Option<(wgpu::TextureFormat, TexturePipeline)>,
    ui_textures: HashMap<String, Option<wgpu::Texture>>,
    clear_color: wgpu::Color,
    /// 画面设置里的天空节：开着时环境照明多一份天空漫反射 IBL。
    sky: SkySettings,
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
            depth: None,
            images: HashMap::new(),
            ui_overlay: None,
            ui_pipeline: None,
            ui_textures: HashMap::new(),
            clear_color: wgpu::Color::BLACK,
            sky: SkySettings::default(),
        }
    }

    /// 设置天空（等价于旧 `project.ron` 的 `render.sky`）。
    pub fn set_sky(&mut self, sky: SkySettings) {
        self.sky = sky.sanitized();
    }

    /// 清屏色也过同一条 ACES 曲线——旧引擎整帧（含天空）都在合成 pass 里过曲线。
    pub fn set_clear_color(&mut self, color: [f64; 4]) {
        let linear = [color[0] as f32, color[1] as f32, color[2] as f32];
        let mapped = super::tone::aces(linear);
        self.clear_color = wgpu::Color {
            r: f64::from(mapped[0]),
            g: f64::from(mapped[1]),
            b: f64::from(mapped[2]),
            a: color[3],
        };
    }

    pub fn meshes(&self) -> &MeshCache {
        &self.meshes
    }
    pub fn gpu_mesh_count(&self) -> usize {
        self.gpu_meshes.len()
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
        let mut stats = self.render_list(context, target, world, list, view_proj, lighting)?;
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
        self.render_list(context, target, world, list, view_proj, lighting)
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

    /// 按给定 view-projection 出 draw：静态实例一批，GPU 蒙皮逐网格。
    /// 拟合并渲染方向光阴影；没有可用光源 / 场景时返回 `None`（不采样阴影）。
    fn prepare_shadow(
        &mut self,
        context: &GpuContext,
        world: &World,
        items: &[super::draw::DrawItem],
    ) -> Result<Option<Mat4>, GpuError> {
        let Some(light) = world.environment().lights.iter().find(|light| {
            light.kind == xgc_world::LightKind::Directional
                && light.is_active()
                && light.casts_shadow
        }) else {
            return Ok(None);
        };
        let bounds = super::shot::scene_bounds(world, &mut self.meshes)
            .map_err(|err| GpuError::Mesh(err.to_string()))?;
        let Some(bounds) = bounds else {
            return Ok(None);
        };
        let Some(matrix) =
            super::shadow::fit_directional_shadow(light.normalized_direction(), &bounds)
        else {
            return Ok(None);
        };
        if self.textures.is_none() {
            self.textures = Some(super::texture_bind::TextureBinding::new(context));
        }
        if let Some(target) = self.shadow.as_ref() {
            target
                .render(
                    context,
                    matrix,
                    items,
                    &mut self.gpu_meshes,
                    &mut self.meshes,
                    self.textures.as_ref().unwrap(),
                )
                .map_err(GpuError::Pipeline)?;
        }
        Ok(Some(matrix))
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
        // 阴影只投静态网格：GPU 蒙皮投射是后续切片。
        let static_items: Vec<super::draw::DrawItem> = list
            .items
            .iter()
            .filter(|item| item.pose.is_none())
            .cloned()
            .collect();
        let shadow_matrix = self.prepare_shadow(context, world, &static_items)?;
        let textures = self.textures.as_ref().unwrap();
        lighting = lighting.with_shadow(shadow_matrix);
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
            let skinned = item.pose.as_ref().is_some_and(|pose| pose.len() <= 64);
            // GPU 蒙皮着色器还没有光照与贴图：带贴图的蒙皮网格先走 CPU 蒙皮 + 实例化，
            // 保证和不蒙皮物体同一套 PBR（等 GPU 蒙皮补齐光照再切回快路径）。
            let textured = skinned
                && self
                    .meshes
                    .get(&item.model)
                    .map_err(|err| GpuError::Mesh(err.to_string()))?
                    .is_some_and(|mesh| mesh.texture.is_some());
            if skinned && !textured {
                gpu_items.push(item);
            } else {
                cpu_items.push(item);
            }
        }
        let cpu_list = super::draw::DrawList { items: cpu_items };
        let cpu_empty = cpu_list.items.is_empty();
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
        if !gpu_items.is_empty() {
            let format = target.format();
            if self
                .skinned
                .as_ref()
                .is_none_or(|(current, _)| *current != format)
            {
                self.skinned = Some((format, context.create_skinned_pipeline(format, true)));
            }
            let pipeline = &self.skinned.as_ref().unwrap().1;
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
                let pose = item.pose.as_deref().unwrap_or(&[]);
                let inverse_bind = mesh
                    .skin
                    .as_ref()
                    .map(|skin| skin.inverse_bind.as_slice())
                    .unwrap_or(&[]);
                let gpu_mesh = self.skin_meshes.upload(context, &item.model, mesh);
                pipeline.draw(
                    context,
                    &SkinnedRequest {
                        texture: target,
                        depth: Some(depth),
                        clear: cpu_empty && index == 0,
                        mesh: gpu_mesh,
                        pose,
                        inverse_bind,
                        model: item.transform,
                        color: item.color,
                        view_proj,
                    },
                )?;
                draw_calls += 1;
            }
        }
        if self.sky.enabled {
            // 一个场景 draw 都没有时，实例化 pass 不会清深度（它提前返回了）——
            // 天空 pass 自己把深度清成 1.0；有几何时深度已被清好，只能 Load。
            self.draw_sky(context, target, view_proj, &lighting, draws == 0);
        }
        Ok(RenderStats {
            draws,
            draw_calls,
            ui_draws: 0,
            passes: usize::from(draws > 0),
        })
    }
}
