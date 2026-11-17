//! UI 覆盖层：背景矩形 / 焦点环 / 文本 / 图片画在已渲染画面之上（不写深度）。
//!
//! 从 `renderer.rs` 拆出来只为守住 600 行预算；行为与拆分前逐位一致。

use crate::gpu::{GpuContext, GpuError};
use crate::image::ImageData;
use crate::mesh::Mesh;
use crate::pipeline::{RoundedMeshDraw, TrianglePipeline};
use crate::text::text_mesh_wrapped;
use crate::ui_draw::{cover_uv, quad_matrix};
use xgc_world::{UiElement, UiInteraction, UiLayout, World};

use super::{scissor_rect, Renderer};

impl Renderer {
    fn overlay_pipeline(
        &mut self,
        context: &GpuContext,
        format: wgpu::TextureFormat,
    ) -> &TrianglePipeline {
        if self.ui_overlay.as_ref().map(|(current, _)| *current) != Some(format) {
            self.ui_overlay = Some((format, context.create_overlay_pipeline(format)));
        }
        &self.ui_overlay.as_ref().unwrap().1
    }

    /// 渲染 UI 背景矩形：在已渲染画面上叠加，不写深度；返回 draw 数。
    pub fn render_ui(
        &mut self,
        context: &GpuContext,
        target: &wgpu::Texture,
        world: &World,
    ) -> Result<usize, GpuError> {
        let viewport = [target.width() as f32, target.height() as f32];
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
        if items.is_empty() {
            return Ok(0);
        }
        let mesh = Mesh::quad();
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
                self.overlay_pipeline(context, target.format())
                    .draw_mesh_rounded(&RoundedMeshDraw {
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
                self.overlay_pipeline(context, target.format())
                    .draw_mesh_rounded(&RoundedMeshDraw {
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
                    self.overlay_pipeline(context, target.format())
                        .draw_mesh_scissored(
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
}
