//! UI 覆盖层：背景矩形 / 焦点环 / 真字体文本 / 图片画在已渲染画面之上（不写深度）。
//!
//! 从 `renderer.rs` 拆出来只为守住 600 行预算。文本这一层从 5x7 位图换成
//! `ab_glyph` 真字体（与旧端同一套系统字体候选），对齐方式来自 `ui.element` 的
//! `text_align` / `text_valign`，字形覆盖率走 alpha 混合的独立管线。

use crate::font::{FontAtlas, GlyphQuad};
use crate::gpu::{GpuContext, GpuError};
use crate::mesh::Mesh;
use crate::pipeline::{RoundedMeshDraw, TrianglePipeline};
use crate::text_draw::{TextDraw, TextPipeline};
use crate::ui_draw::{cover_uv, quad_matrix};
use xgc_world::{UiElement, UiInteraction, UiLayout, World};

use super::{scissor_rect, Renderer};

struct UiItem {
    entity: xgc_base::EntityId,
    rect: [f32; 4],
    clip: [f32; 4],
    color: [f32; 4],
    focused: bool,
    text: String,
    text_size: f32,
    text_color: [f32; 4],
    text_align: String,
    text_valign: String,
    bold: bool,
    wrap: bool,
    padding: f32,
    image_path: String,
    image_mode: String,
    image_slice: f32,
    image_tint: [f32; 4],
    image_fill: f32,
    corner_radius: f32,
    border_color: [f32; 4],
    border_width: f32,
}

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

    /// 惰性加载字体图集并同步到 GPU（图集版本变了才重传）。
    fn ensure_font(&mut self, context: &GpuContext, format: wgpu::TextureFormat) {
        if self.font.is_none() {
            self.font = FontAtlas::load();
        }
        let Some(font) = self.font.as_ref() else {
            return;
        };
        let version = font.version();
        if self.font_texture.as_ref().map(|(current, _)| *current) != Some(version) {
            let texture = context.create_texture_rgba8(
                "xgc-font-atlas",
                font.size(),
                font.size(),
                font.pixels(),
            );
            self.font_texture = Some((version, texture));
        }
        if self.text_pipeline.as_ref().map(|(current, _)| *current) != Some(format) {
            self.text_pipeline = Some((format, context.create_text_pipeline(format)));
        }
    }

    /// 渲染 UI 背景矩形 / 文本 / 图片：在已渲染画面上叠加，不写深度；返回 draw 数。
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
            let Some(entity) = world.entity(key) else {
                return;
            };
            let element = world.get::<UiElement>(key).cloned();
            let (color, text, text_size, text_color, wrap, corner_radius, padding) = element
                .as_ref()
                .map(|element| {
                    (
                        element.background,
                        element.text.clone(),
                        element.text_size,
                        element.text_color,
                        element.wrap,
                        element.corner_radius,
                        element.padding,
                    )
                })
                .unwrap_or(([1.0; 4], String::new(), 20.0, [1.0; 4], true, 0.0, 0.0));
            let (border_color, border_width) = element
                .as_ref()
                .map(|element| (element.border_color, element.border_width))
                .unwrap_or(([0.0; 4], 0.0));
            let (image_path, image_mode, image_slice, image_tint, image_fill) = element
                .as_ref()
                .map(|element| {
                    (
                        element.image_path.clone(),
                        element.image_mode.clone(),
                        element.image_slice,
                        element.image_tint,
                        element.image_fill,
                    )
                })
                .unwrap_or((String::new(), String::new(), 0.0, [1.0; 4], 1.0));
            if color[3] <= 0.0 && border_width <= 0.0 && text.is_empty() && image_path.is_empty() {
                return;
            }
            let focused = world
                .get::<UiInteraction>(key)
                .is_some_and(|state| state.focused);
            let (text_align, text_valign, bold) = element
                .as_ref()
                .map(|element| {
                    (
                        element.text_align.clone(),
                        element.text_valign.clone(),
                        element.bold,
                    )
                })
                .unwrap_or_else(|| ("left".to_string(), "middle".to_string(), false));
            items.push(UiItem {
                entity,
                rect: layout.rect,
                clip: layout.clip,
                color,
                focused,
                text,
                text_size,
                text_color,
                text_align,
                text_valign,
                bold,
                wrap,
                padding,
                image_path,
                image_mode,
                image_slice,
                image_tint,
                image_fill,
                corner_radius,
                border_color,
                border_width,
            });
        });
        items.sort_by_key(|item| item.entity);
        if items.is_empty() {
            return Ok(0);
        }
        let mesh = Mesh::quad();
        let mut draws = 0;
        for item in &items {
            let Some(scissor) = scissor_rect(item.clip, viewport) else {
                continue;
            };
            if item.color[3] > 0.0 || item.border_width > 0.0 {
                self.overlay_pipeline(context, target.format())
                    .draw_mesh_rounded(&RoundedMeshDraw {
                        context,
                        texture: target,
                        mesh: &mesh,
                        color: item.color,
                        border_color: item.border_color,
                        border_width: item.border_width,
                        mvp: Some(quad_matrix(item.rect, viewport)),
                        scissor,
                        rect: item.rect,
                        radius: item.corner_radius,
                    })?;
                draws += 1;
            }
            if item.focused {
                let ring = [
                    item.rect[0] - 1.0,
                    item.rect[1] - 1.0,
                    item.rect[2] + 2.0,
                    item.rect[3] + 2.0,
                ];
                self.overlay_pipeline(context, target.format())
                    .draw_mesh_rounded(&RoundedMeshDraw {
                        context,
                        texture: target,
                        mesh: &mesh,
                        color: [0.0; 4],
                        border_color: [1.0, 0.8, 0.2, 1.0],
                        border_width: 2.0,
                        mvp: Some(quad_matrix(ring, viewport)),
                        scissor,
                        rect: ring,
                        radius: item.corner_radius + 1.0,
                    })?;
                draws += 1;
            }
            if !item.image_path.is_empty() && item.image_fill > 0.0 {
                draws += self.draw_ui_image(context, target, viewport, item, scissor)?;
            }
            if !item.text.is_empty() {
                draws += self.draw_ui_text(context, target, viewport, item, scissor)?;
            }
        }
        Ok(draws)
    }

    fn draw_ui_image(
        &mut self,
        context: &GpuContext,
        target: &wgpu::Texture,
        viewport: [f32; 2],
        item: &UiItem,
        _scissor: [u32; 4],
    ) -> Result<usize, GpuError> {
        if !self.images.contains_key(&item.image_path) {
            let loaded = self.meshes.load_ui_image(&item.image_path);
            self.images.insert(item.image_path.clone(), loaded);
        }
        if !self.ui_textures.contains_key(&item.image_path) {
            let texture = self
                .images
                .get(&item.image_path)
                .and_then(|image| image.as_ref())
                .map(|image| {
                    let mut rgba = Vec::with_capacity(image.rgba.len() * 4);
                    for pixel in &image.rgba {
                        rgba.extend_from_slice(pixel);
                    }
                    context.create_texture_rgba8("xgc-ui-image", image.width, image.height, &rgba)
                });
            self.ui_textures.insert(item.image_path.clone(), texture);
        }
        if self.ui_pipeline.as_ref().map(|(format, _)| *format) != Some(target.format()) {
            self.ui_pipeline = Some((
                target.format(),
                context.create_texture_pipeline(target.format()),
            ));
        }
        let Some((_, pipeline)) = self.ui_pipeline.as_ref() else {
            return Ok(0);
        };
        let Some(Some(image)) = self.ui_textures.get(&item.image_path) else {
            return Ok(0);
        };
        let source = self
            .images
            .get(&item.image_path)
            .and_then(|image| image.as_ref());
        let inset = item.padding.max(0.0) + item.border_width.max(0.0);
        let content = [
            item.rect[0] + inset,
            item.rect[1] + inset,
            (item.rect[2] - inset * 2.0).max(1.0),
            (item.rect[3] - inset * 2.0).max(1.0),
        ];
        let mut draw_rect = content;
        let mut uv = [0.0, 0.0, 1.0, 1.0];
        let rect = content;
        if item.image_mode == "nine_slice" && item.image_slice > 0.0 {
            if let Some(source) = source {
                if source.width > 0 && source.height > 0 && rect[2] > 0.0 && rect[3] > 0.0 {
                    let slice_x = (item.image_slice / source.width as f32).min(0.5);
                    let slice_y = (item.image_slice / source.height as f32).min(0.5);
                    let px = item.image_slice.min(rect[2] * 0.5).max(0.0);
                    let py = item.image_slice.min(rect[3] * 0.5).max(0.0);
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
                    let mut draws = 0;
                    for row in 0..3 {
                        for col in 0..3 {
                            let width = xs[col + 1] - xs[col];
                            let height = ys[row + 1] - ys[row];
                            if width <= 0.0 || height <= 0.0 {
                                continue;
                            }
                            let cell = [xs[col], ys[row], width, height];
                            let cell_uv =
                                [u[col], v[row], u[col + 1] - u[col], v[row + 1] - v[row]];
                            pipeline.draw(
                                context,
                                target,
                                image,
                                quad_matrix(cell, viewport),
                                item.image_tint,
                                cell_uv,
                            )?;
                            draws += 1;
                        }
                    }
                    return Ok(draws);
                }
            }
        }
        if item.image_mode == "preserve" {
            if let Some(source) = source {
                if source.width > 0 && source.height > 0 && rect[2] > 0.0 && rect[3] > 0.0 {
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
        } else if item.image_mode == "cover" {
            if let Some(source) = source {
                uv = cover_uv(
                    [source.width as f32, source.height as f32],
                    [rect[2], rect[3]],
                );
            }
        }
        // 填充比例：从左往右裁剪，矩形与 UV 同步收窄，图案保持比例。
        // 九宫格与 fill 互斥（九宫格分支已提前 return），这里只剩普通拉伸 / 保持宽高比。
        if item.image_fill < 1.0 {
            draw_rect[2] *= item.image_fill;
            uv[2] *= item.image_fill;
        }
        pipeline.draw(
            context,
            target,
            image,
            quad_matrix(draw_rect, viewport),
            item.image_tint,
            uv,
        )?;
        Ok(1)
    }

    fn draw_ui_text(
        &mut self,
        context: &GpuContext,
        target: &wgpu::Texture,
        viewport: [f32; 2],
        item: &UiItem,
        scissor: [u32; 4],
    ) -> Result<usize, GpuError> {
        if self.font.is_none() {
            self.font = FontAtlas::load();
        }
        let Some(font) = self.font.as_mut() else {
            return Ok(0);
        };
        // 与旧端一致：内容区扣掉 padding **和**边框。
        let inset = item.padding.max(0.0) + item.border_width.max(0.0);
        let content = [
            (item.rect[2] - inset * 2.0).max(1.0),
            (item.rect[3] - inset * 2.0).max(1.0),
        ];
        let wrap_width = item.wrap.then_some(content[0]);
        let quads: Vec<GlyphQuad> = font.layout(
            &item.text,
            item.text_size.max(1.0),
            [item.rect[0] + inset, item.rect[1] + inset],
            content,
            wrap_width,
            &item.text_align,
            &item.text_valign,
            item.bold,
        );
        // 先排版（会往图集里写新字形），再同步 GPU 贴图，最后才画 —— 顺序反了
        // 会拿旧图集去采样新字形，整段文字变成一片糊。
        self.ensure_font(context, target.format());
        let Some((_, atlas)) = self.font_texture.as_ref() else {
            return Ok(0);
        };
        let Some((_, pipeline)) = self.text_pipeline.as_ref() else {
            return Ok(0);
        };
        let draws = draw_text_quads(
            pipeline,
            context,
            &TextDraw {
                target,
                atlas,
                viewport,
                quads: &quads,
                color: item.text_color,
                scissor,
            },
        )?;
        Ok(draws)
    }
}

/// 单独成函数：借用 `pipeline` / `atlas` 时不再摸 `self`，省掉借用冲突。
fn draw_text_quads(
    pipeline: &TextPipeline,
    context: &GpuContext,
    request: &TextDraw<'_>,
) -> Result<usize, GpuError> {
    if request.quads.is_empty() {
        return Ok(0);
    }
    pipeline.draw(context, request)?;
    Ok(1)
}
