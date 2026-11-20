//! 时间抗锯齿（TAA）：8 相 Halton 抖动 + 深度反投影 + 邻域钳制的历史混合。
//!
//! 口径与旧端 `xgame-render/src/taa.rs` 逐行对齐：
//!
//! - 抖动是**纯函数**（Halton(2,3) 前 8 项的固定循环，无随机数），所以同一帧号永远
//!   得到同一偏移 —— 截图与回归才能逐字节比对。
//! - 抖动**只**叠在相机投影上：阴影拟合、剔除、自动取景全部用未抖动矩阵，否则阴影
//!   会随帧抖动（旧端同一纪律）。
//! - 历史先被当前帧 3×3 邻域钳制再按 `history_weight` 混合，抑制拖影；首帧 / 重建后
//!   第一帧 / 重投影越界 / 相机背后一律直通当前帧（`blend = 0`）。
//! - **默认关闭**：关着时不建历史纹理、不跑 pass、投影不抖动，帧输出与引入本切片前
//!   逐位一致。开着时 FXAA 让位（`render.exposure` 之后的显示空间只留一种抗锯齿）。
//!
//! 乒乓用两块 `Rgba16Float`：同一张纹理不能既当输入又当输出。

use std::sync::Arc;

use super::camera::{invert, Mat4, IDENTITY};
use super::gpu::GpuContext;

const TAA_WGSL: &str = include_str!("taa.wgsl");
const HISTORY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// 抖动相位个数：旧端同为 8（Halton(2,3) 的前 8 项）。
pub const JITTER_SAMPLES: u32 = 8;
/// 历史权重上限。1.0 会把历史冻住（画面再也不更新），旧端同样在 0.98 封顶。
pub const MAX_HISTORY_WEIGHT: f32 = 0.98;
/// 历史权重的缺省值：本帧颜色只占 10%。
pub const DEFAULT_HISTORY_WEIGHT: f32 = 0.9;
/// 邻域钳制的放大量：1.0 = 恰好用当前帧 3×3 的最小 / 最大通道包住历史。
const NEIGHBORHOOD_SCALE: f32 = 1.0;

/// `TaaParams` 的字节数：两个 mat4 + 4 个 vec4 槽位（末尾按 16 字节对齐）。
const PARAMS_BYTES: usize = 176;

/// TAA 设置：与旧端 `TaaConfig` 逐字段、逐缺省值一致（落盘位置 `render.taa`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TaaSettings {
    /// 总开关。缺省关：关着时整段跳过，画面逐位不变。
    pub enabled: bool,
    /// 历史权重（0 = 完全不用历史，0.98 = 几乎冻结）；只在已有可信上一帧时生效。
    pub history_weight: f32,
}

impl Default for TaaSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            history_weight: DEFAULT_HISTORY_WEIGHT,
        }
    }
}

impl TaaSettings {
    /// 容错读取（同旧端 `TaaConfig::sanitized`）：非有限权重落回缺省值，
    /// 合法值夹进 `[0, 0.98]`。
    pub fn sanitized(self) -> Self {
        let history_weight = if self.history_weight.is_finite() {
            self.history_weight.clamp(0.0, MAX_HISTORY_WEIGHT)
        } else {
            DEFAULT_HISTORY_WEIGHT
        };
        Self {
            enabled: self.enabled,
            history_weight,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// 反射式基数展开：`halton(1, 2) = 0.5`、`halton(2, 2) = 0.25`……
pub(crate) fn halton(mut index: u32, base: u32) -> f32 {
    let mut weight = 1.0f32;
    let mut value = 0.0f32;
    while index > 0 {
        weight /= base as f32;
        value += weight * (index % base) as f32;
        index /= base;
    }
    value
}

/// 第 `frame` 帧的抖动偏移（像素，y 朝下）：相位在 8 个 Halton 样本里循环。
pub(crate) fn jitter_offset(frame: u32) -> [f32; 2] {
    let index = frame % JITTER_SAMPLES + 1;
    [halton(index, 2) - 0.5, halton(index, 3) - 0.5]
}

/// 像素偏移 → 裁剪空间偏移（`ndc = 2 * pixel / size`）。
pub(crate) fn jitter_to_ndc(jitter: [f32; 2], size: (u32, u32)) -> [f32; 2] {
    if size.0 == 0 || size.1 == 0 {
        return [0.0, 0.0];
    }
    [
        2.0 * jitter[0] / size.0 as f32,
        2.0 * jitter[1] / size.1 as f32,
    ]
}

/// 像素偏移 → 纹理坐标偏移。**y 取反**：NDC 的 y 朝上、纹理的 y 朝下；
/// 写反等于历史永远差两个像素，画面只是变糊、不会报错。
pub(crate) fn jitter_to_uv(jitter: [f32; 2], size: (u32, u32)) -> [f32; 2] {
    if size.0 == 0 || size.1 == 0 {
        return [0.0, 0.0];
    }
    [jitter[0] / size.0 as f32, -jitter[1] / size.1 as f32]
}

/// 把裁剪空间偏移叠进投影矩阵：对每一列做 `col.x += ndc.x * col.w`、
/// `col.y += ndc.y * col.w`（等价于透视矩阵只动 `[2][0]` / `[2][1]`，
/// 但写成一般形式对任何投影都成立）。`Mat4` 是列主序 `[f32; 16]`。
pub fn jitter_projection(projection: Mat4, ndc: [f32; 2]) -> Mat4 {
    // 零抖动直接原样返回：`+= 0.0 * w` 会把 `-0.0` 规整成 `+0.0`，破坏
    // "零抖动逐位不变"（旧端同样有这条早退）。
    if ndc == [0.0, 0.0] {
        return projection;
    }
    let mut jittered = projection;
    for column in 0..4 {
        let w = projection[column * 4 + 3];
        jittered[column * 4] += ndc[0] * w;
        jittered[column * 4 + 1] += ndc[1] * w;
    }
    jittered
}

/// 打包 `TaaParams`（与 `taa.wgsl` 逐字段对齐，末尾 8 字节填充到 176）。
#[allow(clippy::too_many_arguments)]
fn params_bytes(
    inv_view_proj: Mat4,
    prev_view_proj: Mat4,
    camera_pos: [f32; 3],
    prev_jitter_uv: [f32; 2],
    texel: [f32; 2],
    blend: f32,
    clamp_scale: f32,
    resolution: [f32; 2],
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(PARAMS_BYTES);
    for value in inv_view_proj {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in prev_view_proj {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    // w = 1：旧端写 1.0，着色器只读 `.xyz`，但口径保持一致。
    for value in [camera_pos[0], camera_pos[1], camera_pos[2], 1.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in prev_jitter_uv {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in texel {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in [blend, clamp_scale] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in resolution {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    debug_assert_eq!(bytes.len(), PARAMS_BYTES);
    bytes
}

/// 一帧待解析的输入：当前颜色 + 深度 + 尺寸（打包传参，参数表才不会过长）。
pub(crate) struct ResolveInput<'a> {
    pub(crate) color: &'a wgpu::TextureView,
    pub(crate) depth: &'a wgpu::TextureView,
    pub(crate) size: (u32, u32),
}

struct TaaPipeline {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
}

struct History {
    size: (u32, u32),
    textures: [Arc<wgpu::Texture>; 2],
    views: [wgpu::TextureView; 2],
    /// 本帧的写入索引；上一帧刚写的那张就是本帧的读取索引。
    write: usize,
}

/// TAA 的全部 GPU 状态；每帧 `begin_frame` → 场景用抖动矩阵 → `resolve`。
///
/// 重新开 TAA 时复用已建好的管线 / 采样器，只重建历史纹理。
pub(crate) struct TaaStage {
    pipeline: TaaPipeline,
    /// 深度附件的采样数（场景 pass 的 MSAA 档）：换档要重建管线与绑定布局。
    sample_count: u32,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    history: Option<History>,
    /// 只增的帧号：决定抖动相位，永远不重置（重置只清历史与 reprojection 状态）。
    frame: u32,
    frames_since_reset: u32,
    jitter: [f32; 2],
    prev_jitter: [f32; 2],
    prev_view_proj: Option<Mat4>,
}

impl TaaStage {
    pub(crate) fn new(context: &GpuContext) -> Self {
        let sample_count = context.scene_sample_count();
        let pipeline = create_pipeline(context, sample_count);
        let sampler = context.device().create_sampler(&wgpu::SamplerDescriptor {
            label: Some("xgc-taa-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-taa-params"),
            size: PARAMS_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            sample_count,
            sampler,
            uniform,
            history: None,
            frame: 0,
            frames_since_reset: 0,
            jitter: [0.0; 2],
            prev_jitter: [0.0; 2],
            prev_view_proj: None,
        }
    }

    /// 场景采样数变了（`Renderer::set_sample_count`）：深度绑定与着色器里的声明
    /// 要跟着换；历史也一并丢掉——它是按另一套采样数解析出来的，混用会拖影。
    pub(crate) fn set_sample_count(&mut self, context: &GpuContext, sample_count: u32) {
        if self.sample_count == sample_count {
            return;
        }
        self.sample_count = sample_count;
        self.pipeline = create_pipeline(context, sample_count);
        self.history = None;
    }

    /// 每帧开头调用：换尺寸时丢弃历史（首帧直通），返回本帧抖动（像素）。
    pub(crate) fn begin_frame(&mut self, size: (u32, u32)) -> [f32; 2] {
        if self
            .history
            .as_ref()
            .is_some_and(|history| history.size != size)
        {
            self.release();
        }
        self.jitter = jitter_offset(self.frame);
        self.frame = self.frame.wrapping_add(1);
        self.jitter
    }

    /// 关掉 TAA 时释放历史纹理（管线 / 采样器留着，避免来回开关反复编译）。
    pub(crate) fn release(&mut self) {
        self.history = None;
        self.frames_since_reset = 0;
        self.prev_view_proj = None;
    }

    /// 相机瞬移 / 切场景时调用：保留历史纹理，只清重投影状态，
    /// 让下一帧 `blend = 0` 直通当前帧（否则会拿旧位置的深度去混，残影 1–2 帧）。
    pub(crate) fn reset_history(&mut self) {
        self.frames_since_reset = 0;
        self.prev_view_proj = None;
    }

    /// 重投影状态是否刚被重置（下一帧会直通）。
    pub(crate) fn reprojection_is_reset(&self) -> bool {
        self.frames_since_reset == 0
    }

    /// 当前历史纹理尺寸（`None` = 没有历史）。
    pub(crate) fn history_size(&self) -> Option<(u32, u32)> {
        self.history.as_ref().map(|history| history.size)
    }

    /// 解析：读当前帧颜色与深度，反投影上一帧，混合历史后写进乒乓的另一张。
    ///
    /// `view_proj` 必须是**未抖动**的相机矩阵：反投影要把它还原成"上一帧真实相机"，
    /// 抖动偏移单独以 `prev_jitter_uv` 补进纹理坐标（旧端同式）。
    /// 返回本帧写好的 HDR 纹理与逻辑 pass 数（1）。
    pub(crate) fn resolve(
        &mut self,
        context: &GpuContext,
        frame: ResolveInput<'_>,
        view_proj: Mat4,
        camera_pos: [f32; 3],
        settings: TaaSettings,
    ) -> (Arc<wgpu::Texture>, usize) {
        let ResolveInput { color, depth, size } = frame;
        let size = (size.0.max(1), size.1.max(1));
        self.ensure_history(context, size);
        let jittered_view_proj = jitter_projection(view_proj, jitter_to_ndc(self.jitter, size));
        let inverse = invert(jittered_view_proj);
        // 首帧、刚重建历史、或矩阵不可逆时不要历史：否则会拿一张对不上的旧图去混。
        let blend =
            if self.prev_view_proj.is_some() && self.frames_since_reset > 0 && inverse.is_some() {
                settings.history_weight.clamp(0.0, MAX_HISTORY_WEIGHT)
            } else {
                0.0
            };
        let texel = [1.0 / size.0 as f32, 1.0 / size.1 as f32];
        let bytes = params_bytes(
            inverse.unwrap_or(IDENTITY),
            self.prev_view_proj.unwrap_or(view_proj),
            camera_pos,
            jitter_to_uv(self.prev_jitter, size),
            texel,
            blend,
            NEIGHBORHOOD_SCALE,
            [size.0 as f32, size.1 as f32],
        );
        context.queue().write_buffer(&self.uniform, 0, &bytes);

        let history = self.history.as_ref().expect("刚确保过历史纹理");
        let read = 1 - history.write;
        let write = history.write;
        let bind = context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xgc-taa-bind"),
                layout: &self.pipeline.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(color),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&history.views[read]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: self.uniform.as_entire_binding(),
                    },
                ],
            });
        let mut encoder =
            context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xgc-taa-encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-taa-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &history.views[write],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // 全屏三角铺满，清屏色只在直通分支写不全时兜底。
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        context.queue().submit(Some(encoder.finish()));

        let history = self.history.as_mut().expect("刚确保过历史纹理");
        history.write = read;
        let texture = Arc::clone(&history.textures[write]);
        self.prev_view_proj = Some(view_proj);
        self.prev_jitter = self.jitter;
        self.frames_since_reset += 1;
        (texture, 1)
    }

    fn ensure_history(&mut self, context: &GpuContext, size: (u32, u32)) {
        if self
            .history
            .as_ref()
            .is_some_and(|history| history.size == size)
        {
            return;
        }
        let create = |label: &str| -> (Arc<wgpu::Texture>, wgpu::TextureView) {
            let texture = Arc::new(context.device().create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HISTORY_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }));
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            (texture, view)
        };
        let (first_texture, first_view) = create("xgc-taa-history-a");
        let (second_texture, second_view) = create("xgc-taa-history-b");
        self.history = Some(History {
            size,
            textures: [first_texture, second_texture],
            views: [first_view, second_view],
            write: 0,
        });
        self.frames_since_reset = 0;
        self.prev_view_proj = None;
    }
}

#[cfg(test)]
mod tests;

fn create_pipeline(context: &GpuContext, depth_sample_count: u32) -> TaaPipeline {
    // 场景 pass 开 MSAA 时深度附件是多采样纹理：声明换 `texture_depth_multisampled_2d`，
    // 绑定的 `textureLoad(depth, pixel, 0)` 第三参在多采样下就是 sample index。
    let source = if depth_sample_count > 1 {
        TAA_WGSL.replace("texture_depth_2d", "texture_depth_multisampled_2d")
    } else {
        TAA_WGSL.to_string()
    };
    let shader = context
        .device()
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xgc-taa"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
    let layout = context
        .device()
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xgc-taa-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: depth_sample_count > 1,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(PARAMS_BYTES as u64),
                    },
                    count: None,
                },
            ],
        });
    let pipeline_layout =
        context
            .device()
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("xgc-taa-pipeline-layout"),
                bind_group_layouts: &[&layout],
                push_constant_ranges: &[],
            });
    let pipeline = context
        .device()
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("xgc-taa-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HISTORY_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
    TaaPipeline { layout, pipeline }
}
