//! 帧渲染入口：DrawList → MeshCache → RenderCache → GPU。

use std::collections::HashMap;

use xgc_world::{UiElement, UiInteraction, UiLayout, World};

use super::assets::MeshCache;
use super::camera::{camera_view_projection, IDENTITY};
use super::draw::build_draw_list;
use super::gpu::{GpuContext, GpuError};
use super::gpu_mesh::GpuMeshCache;
use super::image::ImageData;
use super::instanced::InstancedRequest;
use super::mesh::Mesh;
use super::pipeline::{RenderCache, RoundedMeshDraw};
use super::resources::{ResourceDesc, ResourceUsage};
use super::skinned::{GpuSkinMeshCache, SkinnedPipeline, SkinnedRequest};
use super::text::text_mesh_wrapped;
use super::texture::TexturePipeline;
use super::ui_draw::{cover_uv, quad_matrix};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub draws: usize,
    pub draw_calls: usize,
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
    depth: Option<DepthTarget>,
    images: HashMap<String, Option<ImageData>>,
    ui_pipeline: Option<(wgpu::TextureFormat, TexturePipeline)>,
    ui_textures: HashMap<String, Option<wgpu::Texture>>,
    clear_color: wgpu::Color,
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
            depth: None,
            images: HashMap::new(),
            ui_pipeline: None,
            ui_textures: HashMap::new(),
            clear_color: wgpu::Color::BLACK,
        }
    }

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

    /// 渲染 UI 背景矩形：在已渲染画面上叠加，不写深度；返回 draw 数。
    pub fn render_ui(
        &mut self,
        context: &GpuContext,
        target: &wgpu::Texture,
        world: &World,
    ) -> Result<usize, GpuError> {
        let viewport = [target.width() as f32, target.height() as f32];
        let pipeline = context.create_triangle_pipeline(target.format());
        let mesh = Mesh::quad();
        let mut items = Vec::new();
        world.for_each::<UiLayout>(|key, layout| {
            if !layout.visible {
                return;
            }
            let color = world
                .get::<UiElement>(key)
                .map(|element| element.background)
                .unwrap_or([1.0; 4]);
            let (text, text_size, text_color) = world
                .get::<UiElement>(key)
                .map(|element| (element.text.clone(), element.text_size, element.text_color))
                .unwrap_or_default();
            let image_path = world
                .get::<UiElement>(key)
                .map(|element| element.image_path.clone())
                .unwrap_or_default();
            let image_mode = world
                .get::<UiElement>(key)
                .map(|element| element.image_mode.clone())
                .unwrap_or_default();
            let image_slice = world
                .get::<UiElement>(key)
                .map(|element| element.image_slice)
                .unwrap_or_default();
            let wrap = world
                .get::<UiElement>(key)
                .map(|element| element.wrap)
                .unwrap_or(true);
            let corner_radius = world
                .get::<UiElement>(key)
                .map(|element| element.corner_radius)
                .unwrap_or(0.0);
            if color[3] <= 0.0 && text.is_empty() && image_path.is_empty() {
                return;
            }
            let focused = world
                .get::<UiInteraction>(key)
                .is_some_and(|state| state.focused);
            if let Some(entity) = world.entity(key) {
                items.push((
                    entity,
                    layout.rect,
                    color,
                    focused,
                    text,
                    text_size,
                    text_color,
                    image_path,
                    image_mode,
                    image_slice,
                    layout.clip,
                    wrap,
                    corner_radius,
                ));
            }
        });
        items.sort_by_key(|(entity, _, _, _, _, _, _, _, _, _, _, _, _)| *entity);
        let mut draws = 0;
        for (
            _,
            rect,
            color,
            focused,
            text,
            text_size,
            text_color,
            image_path,
            image_mode,
            image_slice,
            clip,
            wrap,
            corner_radius,
        ) in &items
        {
            let Some(scissor) = scissor_rect(*clip, viewport) else {
                continue;
            };
            if *focused {
                let ring = [rect[0] - 2.0, rect[1] - 2.0, rect[2] + 4.0, rect[3] + 4.0];
                pipeline.draw_mesh_rounded(&RoundedMeshDraw {
                    context,
                    texture: target,
                    mesh: &mesh,
                    color: [1.0, 0.8, 0.2, 1.0],
                    mvp: Some(quad_matrix(ring, viewport)),
                    scissor,
                    rect: ring,
                    radius: *corner_radius + 2.0,
                })?;
                draws += 1;
            }
            if color[3] > 0.0 {
                pipeline.draw_mesh_rounded(&RoundedMeshDraw {
                    context,
                    texture: target,
                    mesh: &mesh,
                    color: *color,
                    mvp: Some(quad_matrix(*rect, viewport)),
                    scissor,
                    rect: *rect,
                    radius: *corner_radius,
                })?;
                draws += 1;
            }
            if !image_path.is_empty() {
                if !self.images.contains_key(image_path) {
                    let loaded = ImageData::load(std::path::Path::new(image_path)).ok();
                    self.images.insert(image_path.clone(), loaded);
                }
                if !self.ui_textures.contains_key(image_path) {
                    let texture = self
                        .images
                        .get(image_path)
                        .and_then(|image| image.as_ref())
                        .map(|image| {
                            let mut rgba = Vec::with_capacity(image.rgba.len() * 4);
                            for pixel in &image.rgba {
                                rgba.extend_from_slice(pixel);
                            }
                            context.create_texture_rgba8(
                                "xgc-ui-image",
                                image.width,
                                image.height,
                                &rgba,
                            )
                        });
                    self.ui_textures.insert(image_path.clone(), texture);
                }
                if self.ui_pipeline.as_ref().map(|(format, _)| *format) != Some(target.format()) {
                    self.ui_pipeline = Some((
                        target.format(),
                        context.create_texture_pipeline(target.format()),
                    ));
                }
                if let (Some((_, pipeline)), Some(Some(image))) =
                    (&self.ui_pipeline, self.ui_textures.get(image_path))
                {
                    let mut draw_rect = *rect;
                    let mut uv = [0.0, 0.0, 1.0, 1.0];
                    let source = self.images.get(image_path).and_then(|image| image.as_ref());
                    if *image_mode == "nine_slice" && *image_slice > 0.0 {
                        if let Some(source) = source {
                            if source.width > 0
                                && source.height > 0
                                && rect[2] > 0.0
                                && rect[3] > 0.0
                            {
                                let slice_x = (*image_slice / source.width as f32).min(0.5);
                                let slice_y = (*image_slice / source.height as f32).min(0.5);
                                let px = (*image_slice).min(rect[2] * 0.5).max(0.0);
                                let py = (*image_slice).min(rect[3] * 0.5).max(0.0);
                                let u = [0.0, slice_x, 1.0 - slice_x, 1.0];
                                let v = [0.0, slice_y, 1.0 - slice_y, 1.0];
                                let xs = [
                                    rect[0],
                                    rect[0] + px,
                                    rect[0] + rect[2] - px,
                                    rect[0] + rect[2],
                                ];
                                let ys = [
                                    rect[1],
                                    rect[1] + py,
                                    rect[1] + rect[3] - py,
                                    rect[1] + rect[3],
                                ];
                                for row in 0..3 {
                                    for col in 0..3 {
                                        let width = xs[col + 1] - xs[col];
                                        let height = ys[row + 1] - ys[row];
                                        if width <= 0.0 || height <= 0.0 {
                                            continue;
                                        }
                                        let cell = [xs[col], ys[row], width, height];
                                        let cell_uv = [
                                            u[col],
                                            v[row],
                                            u[col + 1] - u[col],
                                            v[row + 1] - v[row],
                                        ];
                                        pipeline.draw(
                                            context,
                                            target,
                                            image,
                                            quad_matrix(cell, viewport),
                                            [1.0; 4],
                                            cell_uv,
                                        )?;
                                        draws += 1;
                                    }
                                }
                                continue;
                            }
                        }
                    }
                    if *image_mode == "preserve" {
                        if let Some(source) =
                            self.images.get(image_path).and_then(|image| image.as_ref())
                        {
                            if source.width > 0
                                && source.height > 0
                                && rect[2] > 0.0
                                && rect[3] > 0.0
                            {
                                let source_aspect = source.width as f32 / source.height as f32;
                                let target_aspect = rect[2] / rect[3];
                                if source_aspect > target_aspect {
                                    let height = rect[2] / source_aspect;
                                    draw_rect[1] += (rect[3] - height) * 0.5;
                                    draw_rect[3] = height;
                                } else {
                                    let width = rect[3] * source_aspect;
                                    draw_rect[0] += (rect[2] - width) * 0.5;
                                    draw_rect[2] = width;
                                }
                            }
                        }
                    } else if image_mode == "cover" {
                        if let Some(source) =
                            self.images.get(image_path).and_then(|image| image.as_ref())
                        {
                            uv = cover_uv(
                                [source.width as f32, source.height as f32],
                                [rect[2], rect[3]],
                            );
                        }
                    }
                    pipeline.draw(
                        context,
                        target,
                        image,
                        quad_matrix(draw_rect, viewport),
                        [1.0; 4],
                        uv,
                    )?;
                    draws += 1;
                }
            }
            if !text.is_empty() {
                let pixel = (*text_size / 7.0).max(1.0);
                let max_width = if *wrap {
                    (rect[2] - 4.0).max(1.0)
                } else {
                    f32::MAX
                };
                let text_mesh = text_mesh_wrapped(
                    text,
                    [rect[0] + 2.0, rect[1] + 2.0],
                    pixel,
                    viewport,
                    max_width,
                );
                if !text_mesh.positions.is_empty() {
                    pipeline.draw_mesh_scissored(
                        context,
                        target,
                        &text_mesh,
                        *text_color,
                        None,
                        scissor,
                    )?;
                    draws += 1;
                }
            }
        }
        Ok(draws)
    }

    /// 渲染一帧：抽 DrawList、解析模型、按实体顺序 draw 到目标纹理。
    pub fn render(
        &mut self,
        context: &GpuContext,
        target: &wgpu::Texture,
        world: &World,
    ) -> Result<RenderStats, GpuError> {
        self.render_selected(context, target, world, None)
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
        self.ensure_depth(context, width, height)?;
        let mut list = build_draw_list(world);
        if let Some(selected) = selected {
            for item in &mut list.items {
                if item.entity == selected {
                    item.color = [1.0, 0.82, 0.2, 1.0];
                }
            }
        }
        let view_proj = camera_view_projection(world, width, height).unwrap_or(IDENTITY);
        let depth = &self.depth.as_ref().unwrap().view;
        let draws = list.len();
        let mut cpu_items = Vec::with_capacity(list.items.len());
        let mut gpu_items = Vec::new();
        for item in list.items {
            if item.pose.as_ref().is_some_and(|pose| pose.len() <= 64) {
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
            depth,
            clear_color: self.clear_color,
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
        Ok(RenderStats {
            draws,
            draw_calls,
            passes: usize::from(draws > 0),
        })
    }
}
