//! UI 渲染 pass：把 [`xgame_ui::UiDrawData`] 画到 LDR 显示目标上。
//!
//! # 位置：必须在后处理**之后**
//!
//! UI 颜色是作者按屏幕观感调的，不能参与曝光 / 色调映射 / Bloom —— 否则
//! 调一下场景曝光，血条跟着一起变亮变暗。所以这条 pass 接在
//! `PostFx::render` 之后、直接写 LDR 目标（编辑器视口纹理 / 交换链 / 截图）。
//! 三家宿主（编辑器 / 独立游戏 / 无头截图）走**同一条** pass。
//!
//! # 一条管线画完所有 UI
//!
//! 矩形、图片、字形全部走**同一个**实例化管线，一个实例 80 字节：
//!
//! | 字段 | 用途 |
//! |---|---|
//! | `rect` | 屏幕像素矩形 |
//! | `uv` | 纹理坐标（矩形模式不用） |
//! | `color` | 乘子色（线性） |
//! | `params` | `x` = 圆角半径，`y` = 边框宽度，`z` = 模式 |
//! | `border_color` | 边框色 |
//!
//! 模式 0（矩形）在片元里算圆角矩形的 SDF，模式 1/2 采样纹理（覆盖率 / RGBA）。
//!
//! # 为什么合并成一条管线（这一版最重要的性能决定）
//!
//! UI 的绘制顺序就是语义（后画的盖住先画的），所以"先画所有背景、再画所有
//! 文字"这种按类型重排是**错的** —— 后一个元素的面板会把前一个元素的文字
//! 盖掉。分成"矩形管线 + 精灵管线"时两者交替出现，一屏 900 个元素会切出
//! 1200 次 draw call（实测）。
//!
//! 合并之后矩形与字形用的是**同一张图集**（矩形不采样，但绑定组相同），
//! 于是相邻的矩形与文字天然同批 —— 整屏 UI 落到 **1 次 draw**（只有图片
//! 会打断批次，因为它是另一张纹理）。
//!
//! # sRGB
//!
//! 交换链格式由系统挑，可能是 `Bgra8UnormSrgb` 也可能是 `Bgra8Unorm`。
//! sRGB 目标由硬件在写入时编码（着色器输出线性值）；非 sRGB 目标存的是
//! "已经编码过的显示值"，必须手动编码 —— 与 `postfx::needs_manual_srgb`
//! 同一套约定，漏了就会"游戏画面正常、UI 明显偏暗"。

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use wgpu::util::DeviceExt as _;
use xgame_core::Scene;
use xgame_math::glam::Vec2;
use xgame_ui::{SpriteMode, UiCommand, UiTexture};

use crate::postfx::needs_manual_srgb;

// ---------------------------------------------------------------------------
// 实例布局
// ---------------------------------------------------------------------------

/// 着色器的全局参数。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct UiUniform {
    viewport: [f32; 2],
    /// 1 = 目标是普通（非 sRGB）格式，着色器里手动编码。
    manual_srgb: u32,
    _pad: u32,
}

/// 一个 UI 图元实例（80 字节）。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct UiInstance {
    /// `min.xy, max.xy`（屏幕像素）。
    rect: [f32; 4],
    /// `u0, v0, u1, v1`。
    uv: [f32; 4],
    /// 乘子色（线性）。
    color: [f32; 4],
    /// `x` = 圆角半径，`y` = 边框宽度，`z` = 模式（0 矩形 / 1 覆盖率 / 2 RGBA）。
    params: [f32; 4],
    /// 边框色（线性）。
    border_color: [f32; 4],
}

const UI_ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x4,
    1 => Float32x4,
    2 => Float32x4,
    3 => Float32x4,
    4 => Float32x4
];

/// 模式：实心圆角矩形。
const MODE_RECT: f32 = 0.0;
/// 模式：单通道覆盖率（字形图集）。
const MODE_ALPHA_MASK: f32 = 1.0;
/// 模式：RGBA 图片。
const MODE_RGBA: f32 = 2.0;

// ---------------------------------------------------------------------------
// 着色器
// ---------------------------------------------------------------------------

const UI_WGSL: &str = r#"
struct Uniforms {
    viewport: vec2<f32>,
    manual_srgb: u32,
    _pad: u32,
};
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var ui_sampler: sampler;
@group(1) @binding(1) var ui_texture: texture_2d<f32>;

struct Instance {
    @location(0) rect: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) params: vec4<f32>,
    @location(4) border_color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) border_color: vec4<f32>,
    // 相对矩形中心的像素偏移（片元里线性插值 = 每像素的精确偏移）。
    @location(3) local: vec2<f32>,
    @location(4) half: vec2<f32>,
    @location(5) params: vec2<f32>,
    @location(6) mode: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    // 4 个顶点组成 triangle strip：(0,0) (1,0) (0,1) (1,1)
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let pos = mix(inst.rect.xy, inst.rect.zw, corner);
    var out: VsOut;
    out.clip = vec4<f32>(
        pos.x / u.viewport.x * 2.0 - 1.0,
        1.0 - pos.y / u.viewport.y * 2.0,
        0.0,
        1.0,
    );
    out.uv = mix(inst.uv.xy, inst.uv.zw, corner);
    out.color = inst.color;
    out.border_color = inst.border_color;
    out.half = (inst.rect.zw - inst.rect.xy) * 0.5;
    out.local = pos - (inst.rect.xy + out.half);
    out.params = inst.params.xy;
    out.mode = inst.params.z;
    return out;
}

fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var rgb: vec3<f32>;
    var alpha: f32;

    if (in.mode < 0.5) {
        // -- 圆角矩形：内缩后的圆角盒有符号距离，<0 在内部 --
        let r = min(in.params.x, min(in.half.x, in.half.y));
        let bw = in.params.y;
        let q = abs(in.local) - (in.half - vec2<f32>(r));
        let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;

        // 1px 宽的过渡带做抗锯齿（UI 是像素对齐的，宽带只会糊）。
        let fill = 1.0 - smoothstep(-0.5, 0.5, d);
        rgb = in.color.rgb;
        alpha = in.color.a * fill;
        if (bw > 0.0) {
            let inner = 1.0 - smoothstep(-0.5, 0.5, d + bw);
            let border = max(fill - inner, 0.0);
            let weight = border / max(fill, 1e-5);
            rgb = mix(in.color.rgb, in.border_color.rgb, weight);
            alpha = in.color.a * inner + in.border_color.a * border;
        }
    } else {
        // 非一致控制流里必须用显式 LOD 的采样。
        let texel = textureSampleLevel(ui_texture, ui_sampler, in.uv, 0.0);
        if (in.mode < 1.5) {
            // 字形：只有覆盖率，颜色完全来自实例色。
            rgb = in.color.rgb;
            alpha = in.color.a * texel.r;
        } else {
            // 图片：sRGB 纹理由硬件解码成线性，直接相乘。
            rgb = in.color.rgb * texel.rgb;
            alpha = in.color.a * texel.a;
        }
    }

    if (u.manual_srgb == 1u) {
        rgb = srgb_encode(rgb);
    }
    return vec4<f32>(rgb, alpha);
}
"#;

// ---------------------------------------------------------------------------
// 纹理
// ---------------------------------------------------------------------------

struct UiTextureEntry {
    width: u32,
    height: u32,
    bind_group: wgpu::BindGroup,
    /// 持有纹理本体，避免视图悬垂。
    _texture: wgpu::Texture,
}

/// 一帧的绘制统计（性能面板 / 基准）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiStats {
    /// 矩形图元数。
    pub quads: usize,
    /// 精灵图元数。
    pub sprites: usize,
    /// 真正的 draw call 数（相邻同纹理的图元会合并）。
    pub draws: usize,
    /// 参与绘制的元素数。
    pub elements: usize,
}

/// UI 渲染器：持有绘制数据（CPU）与 GPU 资源。
pub struct UiRenderer {
    painter: xgame_ui::Painter,
    output_format: wgpu::TextureFormat,
    manual_srgb: bool,

    uniform: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    texture_bgl: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,

    /// 本帧的实例数据（矩形 + 精灵，按绘制顺序）。
    instances: Vec<UiInstance>,
    /// 实例区间 → 纹理。相邻且同纹理的区间在录制时合并成一次 draw。
    runs: Vec<(Range<u32>, UiTexture)>,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,

    sampler: wgpu::Sampler,
    atlas_texture: Option<wgpu::Texture>,
    atlas_bg: Option<wgpu::BindGroup>,
    atlas_size: u32,
    atlas_version: u64,

    images: Vec<UiTextureEntry>,
    image_index: HashMap<String, usize>,

    signature: u64,
    prepared: bool,
    stats: UiStats,
}

impl UiRenderer {
    /// 建好管线与空缓冲。
    pub fn new(device: &wgpu::Device, output_format: wgpu::TextureFormat) -> Self {
        let manual_srgb = needs_manual_srgb(output_format);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui_shader"),
            source: wgpu::ShaderSource::Wgsl(UI_WGSL.into()),
        });

        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ui_uniform"),
            contents: bytemuck::bytes_of(&UiUniform {
                viewport: [1.0, 1.0],
                manual_srgb: u32::from(manual_srgb),
                _pad: 0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui_uniform_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui_uniform_bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });

        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui_texture_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui_pipeline_layout"),
            bind_group_layouts: &[&uniform_bgl, &texture_bgl],
            push_constant_ranges: &[],
        });

        let blend = Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<UiInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &UI_ATTRS,
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: output_format,
                    // 直通 alpha 混合（非预乘）：混合在 sRGB 目标上由硬件
                    // 在线性空间完成。
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui_instances"),
            size: std::mem::size_of::<UiInstance>() as u64 * 256,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ui_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Self {
            painter: xgame_ui::Painter::new(),
            output_format,
            manual_srgb,
            uniform,
            uniform_bg,
            texture_bgl,
            pipeline,
            instances: Vec::new(),
            runs: Vec::new(),
            instance_buffer,
            instance_capacity: 256,
            sampler,
            atlas_texture: None,
            atlas_bg: None,
            atlas_size: 0,
            atlas_version: u64::MAX,
            images: Vec::new(),
            image_index: HashMap::new(),
            signature: 0,
            prepared: false,
            stats: UiStats::default(),
        }
    }

    /// 输出目标格式。
    pub fn output_format(&self) -> wgpu::TextureFormat {
        self.output_format
    }

    /// 当前绘制数据（测试 / 调试用）。
    pub fn draw(&self) -> &xgame_ui::UiDrawData {
        self.painter.draw()
    }

    /// 本帧统计。
    pub fn stats(&self) -> UiStats {
        self.stats
    }

    /// 文本引擎（字体是否可用等）。
    pub fn painter(&self) -> &xgame_ui::Painter {
        &self.painter
    }

    /// 丢掉所有图片纹理缓存（切换项目 / 资产改动后调用）。
    pub fn clear_images(&mut self) {
        self.images.clear();
        self.image_index.clear();
    }

    /// 重建绘制数据，返回本帧 UI 的**内容签名**。
    ///
    /// 宿主的指纹门控把它混进场景签名里 —— 否则"场景没动、只有 UI 在变"
    /// （悬停变色、血量数字跳动）会被整帧跳过，UI 看起来像卡住了。
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        viewport: (u32, u32),
        project_root: &Path,
    ) -> u64 {
        if viewport.0 == 0 || viewport.1 == 0 {
            self.prepared = false;
            self.signature = 0;
            self.stats = UiStats::default();
            return 0;
        }
        let size = Vec2::new(viewport.0 as f32, viewport.1 as f32);

        // 1) 先把本帧要用到的图片纹理准备好（`keep_aspect` 需要原始尺寸）。
        let mut needed: Vec<String> = Vec::new();
        for entity in &scene.entities {
            let Some(image) = entity.ui.as_ref().and_then(|ui| ui.image.as_ref()) else {
                continue;
            };
            if image.path.is_empty() || needed.iter().any(|p| p == &image.path) {
                continue;
            }
            needed.push(image.path.clone());
        }
        for path in &needed {
            if !self.image_index.contains_key(path) {
                self.load_image(device, queue, path, project_root);
            }
        }
        let sizes: Vec<(String, Vec2)> = needed
            .iter()
            .filter_map(|path| {
                let entry = self.image_index.get(path)?;
                let entry = self.images.get(*entry)?;
                Some((
                    path.clone(),
                    Vec2::new(entry.width as f32, entry.height as f32),
                ))
            })
            .collect();
        for (path, size) in sizes {
            self.painter.set_image_size(&path, size);
        }

        // 2) 排版 + 合批
        let draw = self.painter.build(scene, size);
        self.signature = draw_signature(draw);
        self.stats = UiStats {
            quads: draw.quads.len(),
            sprites: draw.sprites.len(),
            draws: coalesce_draws(draw.commands.as_slice()).len(),
            elements: draw.element_count,
        };
        self.prepared = !draw.commands.is_empty();
        self.signature
    }

    /// 录制 UI pass（必须在 `PostFx::render` 之后调用）。
    ///
    /// 返回是否真的画了东西（`false` = 本帧没有 UI，或还没 `prepare`）。
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: (u32, u32),
    ) -> bool {
        if !self.prepared || viewport.0 == 0 || viewport.1 == 0 {
            return false;
        }

        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&UiUniform {
                viewport: [viewport.0 as f32, viewport.1 as f32],
                manual_srgb: u32::from(self.manual_srgb),
                _pad: 0,
            }),
        );

        let atlas_version = self.painter.draw().atlas_version;
        let atlas_size = self.painter.text().atlas().size();
        self.sync_atlas(device, queue, atlas_size, atlas_version);

        // -- 实例数据：矩形 + 精灵按绘制顺序排成一条流 --
        self.instances.clear();
        self.runs.clear();
        {
            let draw = self.painter.draw();
            for command in &draw.commands {
                let texture = match *command {
                    UiCommand::Rects { start, count } => {
                        for quad in &draw.quads[start as usize..(start + count) as usize] {
                            self.instances.push(UiInstance {
                                rect: [
                                    quad.rect.min.x,
                                    quad.rect.min.y,
                                    quad.rect.max.x,
                                    quad.rect.max.y,
                                ],
                                uv: [0.0; 4],
                                color: quad.color.to_array(),
                                params: [quad.corner_radius, quad.border_width, MODE_RECT, 0.0],
                                border_color: quad.border_color.to_array(),
                            });
                        }
                        // 矩形不采样纹理，但绑定组要一致才能与文字同批 ——
                        // 用图集，于是"背景 + 文字"天然是一次 draw。
                        UiTexture::Atlas
                    }
                    UiCommand::Sprites {
                        start,
                        count,
                        texture,
                    } => {
                        for sprite in &draw.sprites[start as usize..(start + count) as usize] {
                            self.instances.push(UiInstance {
                                rect: [
                                    sprite.rect.min.x,
                                    sprite.rect.min.y,
                                    sprite.rect.max.x,
                                    sprite.rect.max.y,
                                ],
                                uv: sprite.uv,
                                color: sprite.color.to_array(),
                                params: [
                                    0.0,
                                    0.0,
                                    match sprite.mode {
                                        SpriteMode::AlphaMask => MODE_ALPHA_MASK,
                                        SpriteMode::Rgba => MODE_RGBA,
                                    },
                                    0.0,
                                ],
                                border_color: [0.0; 4],
                            });
                        }
                        texture
                    }
                };
                push_run(&mut self.runs, texture, self.instances.len() as u32);
            }
        }

        if self.instances.is_empty() {
            return false;
        }
        if let Some(buffer) = upload_instances(
            device,
            queue,
            &self.instance_buffer,
            &mut self.instance_capacity,
            "ui_instances",
            &self.instances,
        ) {
            self.instance_buffer = buffer;
        }

        let Some(atlas_bg) = self.atlas_bg.as_ref() else {
            return false;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ui_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.uniform_bg, &[]);
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));

        let mut bound: Option<UiTexture> = None;
        for (range, texture) in &self.runs {
            if bound != Some(*texture) {
                let bind_group = match texture {
                    UiTexture::Atlas => Some(atlas_bg),
                    UiTexture::Image(index) => {
                        self.images.get(*index as usize).map(|entry| &entry.bind_group)
                    }
                };
                let Some(bind_group) = bind_group else {
                    continue;
                };
                pass.set_bind_group(1, bind_group, &[]);
                bound = Some(*texture);
            }
            pass.draw(0..4, range.clone());
        }
        true
    }

    /// 上传 / 重建字形图集。
    fn sync_atlas(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, size: u32, version: u64) {
        if self.atlas_texture.is_some() && self.atlas_size == size && self.atlas_version == version
        {
            return;
        }
        let recreate = self.atlas_texture.is_none() || self.atlas_size != size;
        if recreate {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("ui_glyph_atlas"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // R8：字形只需要覆盖率，显存是 RGBA 的 1/4。
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.atlas_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ui_atlas_bg"),
                layout: &self.texture_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                ],
            }));
            self.atlas_texture = Some(texture);
            self.atlas_size = size;
        }
        let Some(texture) = self.atlas_texture.as_ref() else {
            return;
        };
        let pixels = self.painter.text().atlas().pixels();
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(size),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        self.atlas_version = version;
    }

    /// 加载一张图片（失败时也登记一张 1×1 透明纹理，避免每帧重试读盘）。
    fn load_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &str,
        project_root: &Path,
    ) {
        let full = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            project_root.join(path)
        };
        let loaded = match load_rgba8(&full) {
            Ok((width, height, pixels)) => Some((width, height, pixels)),
            Err(err) => {
                log::warn!("UI 图片 '{path}' 加载失败: {err}");
                None
            }
        };
        let (width, height, pixels) = loaded.unwrap_or((1, 1, vec![0, 0, 0, 0]));
        let (texture, bind_group) = create_image_texture(
            device,
            queue,
            &self.texture_bgl,
            &self.sampler,
            &format!("ui_image:{path}"),
            width,
            height,
            &pixels,
        );
        self.image_index.insert(path.to_owned(), self.images.len());
        self.images.push(UiTextureEntry {
            width,
            height,
            bind_group,
            _texture: texture,
        });
    }
}

/// 把命令序列压成"同纹理的连续区间"。
///
/// `prepare` 用它算 draw call 读数，`render` 用同样的规则切区间 —— 一处规则，
/// 两处不会漂移。
fn coalesce_draws(commands: &[UiCommand]) -> Vec<(Range<u32>, UiTexture)> {
    let mut runs: Vec<(Range<u32>, UiTexture)> = Vec::new();
    let mut offset = 0u32;
    for command in commands {
        let (count, texture) = match *command {
            UiCommand::Rects { count, .. } => (count, UiTexture::Atlas),
            UiCommand::Sprites { count, texture, .. } => (count, texture),
        };
        push_run_at(&mut runs, texture, offset, count);
        offset += count;
    }
    runs
}

fn push_run(runs: &mut Vec<(Range<u32>, UiTexture)>, texture: UiTexture, end: u32) {
    let start = runs.last().map(|(range, _)| range.end).unwrap_or(0);
    push_run_at(runs, texture, start, end.saturating_sub(start));
}

fn push_run_at(
    runs: &mut Vec<(Range<u32>, UiTexture)>,
    texture: UiTexture,
    start: u32,
    count: u32,
) {
    if count == 0 {
        return;
    }
    match runs.last_mut() {
        Some((range, last)) if *last == texture && range.end == start => range.end += count,
        _ => runs.push((start..start + count, texture)),
    }
}

/// 建一张 UI 图片纹理与它的绑定组。
///
/// 参数多但不抽结构体：这是一次性构造，拆成结构体只会让调用点更长。
#[allow(clippy::too_many_arguments)]
fn create_image_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    label: &str,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> (wgpu::Texture, wgpu::BindGroup) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // 图片是美术给的 sRGB 资产：硬件解码成线性，着色器里直接乘实例色。
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    upload_padded(queue, &texture, width.max(1), height.max(1), pixels);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ui_image_bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&view),
            },
        ],
    });
    (texture, bind_group)
}

/// 上传实例数据；容量不够时**新建**缓冲区并返回它（`None` = 沿用旧的）。
fn upload_instances<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    capacity: &mut usize,
    label: &str,
    data: &[T],
) -> Option<wgpu::Buffer> {
    let bytes = bytemuck::cast_slice(data);
    if *capacity < data.len() {
        let next = data.len().next_power_of_two().max(256);
        let created = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (next * std::mem::size_of::<T>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        *capacity = next;
        queue.write_buffer(&created, 0, bytes);
        Some(created)
    } else {
        queue.write_buffer(buffer, 0, bytes);
        None
    }
}

/// 读一张图片为 RGBA8。
fn load_rgba8(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let image = image::open(path).map_err(|e| e.to_string())?;
    let rgba = image.to_rgba8();
    Ok((rgba.width(), rgba.height(), rgba.into_raw()))
}

/// `write_texture` 需要 256 字节的行对齐，这里补零后上传。
fn upload_padded(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    pixels: &[u8],
) {
    let unpadded = (width * 4) as usize;
    let padded = unpadded.div_ceil(256) * 256;
    if padded == unpadded {
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(unpadded as u32),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        return;
    }
    let mut staging = vec![0u8; padded * height as usize];
    for y in 0..height as usize {
        let src = y * unpadded;
        let dst = y * padded;
        staging[dst..dst + unpadded].copy_from_slice(&pixels[src..src + unpadded]);
    }
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &staging,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(padded as u32),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// 绘制数据的内容签名（用于宿主的指纹门控）。
///
/// 只哈希"看得见的量"：矩形、颜色、UV、批顺序、图集版本。刻意不哈希
/// `viewport`（它已经在场景签名里）与元素数（会随 quads/sprites 一起变）。
fn draw_signature(draw: &xgame_ui::UiDrawData) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |value: u64| {
        hash ^= value;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    mix(draw.quads.len() as u64);
    mix(draw.sprites.len() as u64);
    mix(draw.commands.len() as u64);
    mix(draw.atlas_version);
    for quad in &draw.quads {
        for v in [
            quad.rect.min.x,
            quad.rect.min.y,
            quad.rect.max.x,
            quad.rect.max.y,
            quad.color.r,
            quad.color.g,
            quad.color.b,
            quad.color.a,
            quad.corner_radius,
            quad.border_width,
            quad.border_color.r,
            quad.border_color.g,
            quad.border_color.b,
            quad.border_color.a,
        ] {
            mix(v.to_bits() as u64);
        }
    }
    for sprite in &draw.sprites {
        for v in [
            sprite.rect.min.x,
            sprite.rect.min.y,
            sprite.rect.max.x,
            sprite.rect.max.y,
            sprite.uv[0],
            sprite.uv[1],
            sprite.uv[2],
            sprite.uv[3],
            sprite.color.r,
            sprite.color.g,
            sprite.color.b,
            sprite.color.a,
        ] {
            mix(v.to_bits() as u64);
        }
        mix(match sprite.texture {
            UiTexture::Atlas => 0,
            UiTexture::Image(index) => index as u64 + 1,
        });
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 矩形与字形共用图集：整段交替的"背景 / 文字 / 背景 / 文字"必须合成一次 draw。
    #[test]
    fn rects_and_glyphs_share_one_draw() {
        let commands = vec![
            UiCommand::Rects { start: 0, count: 2 },
            UiCommand::Sprites {
                start: 0,
                count: 30,
                texture: UiTexture::Atlas,
            },
            UiCommand::Rects { start: 2, count: 1 },
            UiCommand::Sprites {
                start: 30,
                count: 12,
                texture: UiTexture::Atlas,
            },
        ];
        let runs = coalesce_draws(&commands);
        assert_eq!(runs.len(), 1, "同纹理的相邻区间必须合并: {runs:?}");
        assert_eq!(runs[0].0, 0..45);
    }

    /// 图片是另一张纹理，必须切断批次 —— 但两侧各自仍要合并。
    #[test]
    fn image_breaks_the_batch() {
        let commands = vec![
            UiCommand::Rects { start: 0, count: 1 },
            UiCommand::Sprites {
                start: 0,
                count: 1,
                texture: UiTexture::Image(0),
            },
            UiCommand::Rects { start: 1, count: 1 },
        ];
        let runs = coalesce_draws(&commands);
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0], (0..1, UiTexture::Atlas));
        assert_eq!(runs[1], (1..2, UiTexture::Image(0)));
        assert_eq!(runs[2], (2..3, UiTexture::Atlas));
    }

    #[test]
    fn empty_commands_produce_no_runs() {
        assert!(coalesce_draws(&[]).is_empty());
    }
}
