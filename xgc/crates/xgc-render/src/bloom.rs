//! Bloom：HDR 亮部溢出的柔光。
//!
//! 场景已经在线性 `Rgba16Float` 里合成（见 [`super::renderer::hdr`]），这里只做
//! "亮部往外溢一点"：
//!
//! ```text
//!  HDR ──亮部提取+½降采样──▶ a0 ──降采样──▶ a1 ──降采样──▶ a2
//!  每级各跑一次横向 / 纵向可分离高斯，结果留在 a0..a2；
//!  tone-map 的合成 pass 读这三级，乘权重与强度后加进**曲线前**的线性颜色。
//! ```
//!
//! 口径与旧端 `postfx` 的 Bloom 链相同：阈值判在曝光后（XGC 没有曝光旋钮，
//! 等价于曝光恒为 1.0）、软膝避免亮斑边缘的硬边、按最大通道判亮度（彩灯的
//! 光晕是彩色的）、三级权重 `[0.5, 0.3, 0.2]`。
//!
//! **默认关闭（`intensity == 0`）**：Bloom 是风格参数而不是正确性参数（对比：
//! 色调映射必须回答"高光怎么落进显示区间"）。关着时整段跳过 —— 不建纹理、
//! 不跑 pass，只把 tone-map 的强度写成 0，画面与上一刀逐位相同。
//!
//! 乒乓用两块纹理（`a` / `b`）：同一张纹理不能既当输入又当输出。

use crate::gpu::GpuContext;

const BLOOM_WGSL: &str = include_str!("bloom.wgsl");
const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// 光晕级数（½ / ¼ / ⅛）。
pub const BLOOM_LEVELS: usize = 3;
/// 三级光晕的合成权重（和为 1：与强度相乘后是"光晕总能量"的线性度量）。
pub const BLOOM_WEIGHTS: [f32; BLOOM_LEVELS] = [0.5, 0.3, 0.2];
/// 一条 Bloom 链记几个逻辑渲染阶段：1 次亮部提取 + 2 次降采样 + 每级 2 次模糊。
pub const BLOOM_PASSES: usize = 1 + (BLOOM_LEVELS - 1) + 2 * BLOOM_LEVELS;

/// `BloomParams` 的字节数：4 × f32 + 2 × vec2<f32>。
const PARAMS_BYTES: usize = 32;

/// 光晕参数：与旧端 `BloomConfig` 逐字段、逐缺省值一致。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BloomSettings {
    /// 光晕强度（叠加比例，0 = 关闭）。
    pub intensity: f32,
    /// 亮部提取阈值（曝光后的线性亮度；1.0 ≈ 显示白点）。
    pub threshold: f32,
    /// 软膝半宽：阈值附近多宽的一段"淡入"，避免亮斑边缘出现硬边。
    pub knee: f32,
}

impl Default for BloomSettings {
    fn default() -> Self {
        Self {
            intensity: 0.0,
            threshold: 1.0,
            knee: 0.6,
        }
    }
}

impl BloomSettings {
    /// 光晕强度的上限：再亮只是整幅糊成一片白，没有可用的画面。
    pub const MAX_INTENSITY: f32 = 10.0;
    /// 阈值上限：场景亮度标定是"白面 ≈ 1.0"，100 已远超任何真实内容。
    pub const MAX_THRESHOLD: f32 = 100.0;
    /// 软膝半宽上限（线性亮度单位）。
    pub const MAX_KNEE: f32 = 4.0;

    /// 容错读取（同旧端 `BloomConfig::sanitized` 的契约）：NaN / 无穷 / 负数
    /// 落回默认值，合法但离谱的量级夹进安全范围。
    pub fn sanitized(self) -> Self {
        let fallback = Self::default();
        let clamp = |value: f32, max: f32, default: f32| {
            if value.is_finite() {
                value.clamp(0.0, max)
            } else {
                default
            }
        };
        Self {
            intensity: clamp(self.intensity, Self::MAX_INTENSITY, fallback.intensity),
            threshold: clamp(self.threshold, Self::MAX_THRESHOLD, fallback.threshold),
            knee: clamp(self.knee, Self::MAX_KNEE, fallback.knee),
        }
    }

    /// `intensity == 0` 即关闭：不设独立开关，两个字段表达同一状态迟早互相矛盾。
    pub fn is_enabled(&self) -> bool {
        self.intensity > 0.0
    }
}

/// 第 `level` 级的尺寸：a0 是 ½，a1 是 ¼，a2 是 ⅛；极小目标退到 1 像素。
fn level_size(width: u32, height: u32, level: usize) -> (u32, u32) {
    (
        (width.max(1) >> (level + 1)).max(1),
        (height.max(1) >> (level + 1)).max(1),
    )
}

fn params_bytes(threshold: f32, knee: f32, texel: [f32; 2], direction: [f32; 2]) -> Vec<u8> {
    // 曝光恒为 1.0：XGC 的画面临界值都按"白面 ≈ 1.0"标定，没有曝光旋钮。
    let fields = [
        1.0,
        threshold,
        knee,
        0.0,
        texel[0],
        texel[1],
        direction[0],
        direction[1],
    ];
    let mut bytes = Vec::with_capacity(PARAMS_BYTES);
    for value in fields {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    debug_assert_eq!(bytes.len(), PARAMS_BYTES);
    bytes
}

fn uniform_buffer(context: &GpuContext, label: &str, bytes: &[u8]) -> wgpu::Buffer {
    let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: PARAMS_BYTES as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    context.queue().write_buffer(&buffer, 0, bytes);
    buffer
}

fn bind_source(
    context: &GpuContext,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    view: &wgpu::TextureView,
    params: &wgpu::Buffer,
) -> wgpu::BindGroup {
    context
        .device()
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params.as_entire_binding(),
                },
            ],
        })
}

fn record_fullscreen(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::RenderPipeline,
    bind: &wgpu::BindGroup,
    target: &wgpu::TextureView,
    label: &str,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind, &[]);
    pass.draw(0..3, 0..1);
}

/// 三个 pass 的管线：共用一条布局（单纹理 + 采样器 + uniform），只是片元入口不同。
/// 目标格式全程 `Rgba16Float`，与显示目标格式无关，因此不需要按格式缓存。
struct Pipelines {
    layout: wgpu::BindGroupLayout,
    bright: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
}

impl Pipelines {
    fn new(context: &GpuContext) -> Self {
        let layout = context
            .device()
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("xgc-bloom-layout"),
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
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
        let shader = context
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("xgc-bloom"),
                source: wgpu::ShaderSource::Wgsl(BLOOM_WGSL.into()),
            });
        let pipeline_layout =
            context
                .device()
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("xgc-bloom-pipeline-layout"),
                    bind_group_layouts: &[&layout],
                    push_constant_ranges: &[],
                });
        let pipeline = |label: &str, entry: &str| {
            context
                .device()
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
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
                        entry_point: Some(entry),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: HDR_FORMAT,
                            blend: Some(wgpu::BlendState::REPLACE),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview: None,
                    cache: None,
                })
        };
        Self {
            bright: pipeline("xgc-bloom-bright", "fs_bright"),
            down: pipeline("xgc-bloom-down", "fs_down"),
            blur: pipeline("xgc-bloom-blur", "fs_blur"),
            layout,
        }
    }
}

fn level_view(context: &GpuContext, label: &str, (width, height): (u32, u32)) -> wgpu::TextureView {
    let texture = context.device().create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HDR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// 一次分辨率 + 一组提取参数下的三级光晕纹理与它们的绑定。
///
/// 阈值 / 软膝只在亮部提取里用，改了就得重跑整条链，所以它们是链的一部分；
/// 强度只影响 tone-map 的合成，不进这里（改了不用重建纹理）。
struct Chain {
    width: u32,
    height: u32,
    threshold: f32,
    knee: f32,
    /// 模糊结果（tone-map 合成读这三张）。
    a: [wgpu::TextureView; BLOOM_LEVELS],
    /// 乒乓中间结果。
    b: [wgpu::TextureView; BLOOM_LEVELS],
    down: [wgpu::BindGroup; BLOOM_LEVELS - 1],
    blur_h: [wgpu::BindGroup; BLOOM_LEVELS],
    blur_v: [wgpu::BindGroup; BLOOM_LEVELS],
    bright_params: wgpu::Buffer,
}

impl Chain {
    fn new(
        context: &GpuContext,
        pipelines: &Pipelines,
        sampler: &wgpu::Sampler,
        width: u32,
        height: u32,
        settings: BloomSettings,
    ) -> Self {
        let size = (width.max(1), height.max(1));
        let a: [wgpu::TextureView; BLOOM_LEVELS] = std::array::from_fn(|level| {
            level_view(
                context,
                &format!("xgc-bloom-a{level}"),
                level_size(size.0, size.1, level),
            )
        });
        let b: [wgpu::TextureView; BLOOM_LEVELS] = std::array::from_fn(|level| {
            level_view(
                context,
                &format!("xgc-bloom-b{level}"),
                level_size(size.0, size.1, level),
            )
        });
        let bright_params = uniform_buffer(
            context,
            "xgc-bloom-bright-params",
            &params_bytes(settings.threshold, settings.knee, [0.0; 2], [0.0; 2]),
        );
        // 降采样不读 uniform，但三个 pass 共用一条布局 —— 绑一条全中性缓冲，
        // 比自己再维护一套布局 + 一个 WGSL 结构便宜。
        let zeroed = uniform_buffer(context, "xgc-bloom-zero-params", &[0u8; PARAMS_BYTES]);
        // 模糊的步长与方向建链时定死（每级 texel 不同，方向只有横 / 纵两种）。
        let blur_buffers = |direction: [f32; 2]| -> [wgpu::Buffer; BLOOM_LEVELS] {
            std::array::from_fn(|level| {
                let (level_width, level_height) = level_size(size.0, size.1, level);
                uniform_buffer(
                    context,
                    "xgc-bloom-blur-params",
                    &params_bytes(
                        0.0,
                        0.0,
                        [1.0 / level_width as f32, 1.0 / level_height as f32],
                        direction,
                    ),
                )
            })
        };
        let blur_h_buffers = blur_buffers([1.0, 0.0]);
        let blur_v_buffers = blur_buffers([0.0, 1.0]);
        let down: [wgpu::BindGroup; BLOOM_LEVELS - 1] = std::array::from_fn(|level| {
            bind_source(
                context,
                "xgc-bloom-down-bind",
                &pipelines.layout,
                sampler,
                &a[level],
                &zeroed,
            )
        });
        let blur_h: [wgpu::BindGroup; BLOOM_LEVELS] = std::array::from_fn(|level| {
            bind_source(
                context,
                "xgc-bloom-blur-h-bind",
                &pipelines.layout,
                sampler,
                &a[level],
                &blur_h_buffers[level],
            )
        });
        let blur_v: [wgpu::BindGroup; BLOOM_LEVELS] = std::array::from_fn(|level| {
            bind_source(
                context,
                "xgc-bloom-blur-v-bind",
                &pipelines.layout,
                sampler,
                &b[level],
                &blur_v_buffers[level],
            )
        });
        Self {
            width: size.0,
            height: size.1,
            threshold: settings.threshold,
            knee: settings.knee,
            a,
            b,
            down,
            blur_h,
            blur_v,
            bright_params,
        }
    }
}

/// 光晕链：按"目标尺寸 + 提取参数"缓存，只在变了时重建。
pub(crate) struct BloomStage {
    sampler: wgpu::Sampler,
    pipelines: Option<Pipelines>,
    chain: Option<Chain>,
}

impl BloomStage {
    pub(crate) fn new(context: &GpuContext) -> Self {
        let sampler = context.device().create_sampler(&wgpu::SamplerDescriptor {
            label: Some("xgc-bloom-sampler"),
            // 模糊在屏幕边界处向外多看几个 texel，夹到边缘才不会把对侧的光晕卷进来。
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            sampler,
            pipelines: None,
            chain: None,
        }
    }

    /// 跑完整条链；返回记了几个逻辑渲染阶段（关着时调用方根本不会进来）。
    pub(crate) fn render(
        &mut self,
        context: &GpuContext,
        source: &wgpu::Texture,
        settings: BloomSettings,
    ) -> usize {
        let (width, height) = (source.width().max(1), source.height().max(1));
        if self.pipelines.is_none() {
            self.pipelines = Some(Pipelines::new(context));
        }
        let stale = self.chain.as_ref().is_none_or(|chain| {
            chain.width != width
                || chain.height != height
                || chain.threshold.to_bits() != settings.threshold.to_bits()
                || chain.knee.to_bits() != settings.knee.to_bits()
        });
        if stale {
            let pipelines = self.pipelines.as_ref().expect("上一行刚建过管线");
            let chain = Chain::new(context, pipelines, &self.sampler, width, height, settings);
            self.chain = Some(chain);
        }
        let pipelines = self.pipelines.as_ref().expect("刚建过管线");
        let chain = self.chain.as_ref().expect("刚建过链");
        // 亮部提取的源是 HDR 场景：它随窗口尺寸重建，绑定每帧建一次。
        let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
        let bright_bind = bind_source(
            context,
            "xgc-bloom-bright-bind",
            &pipelines.layout,
            &self.sampler,
            &source_view,
            &chain.bright_params,
        );
        let mut encoder =
            context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xgc-bloom-encoder"),
                });
        // 顺序固定：亮部提取 → 降采样（写高一级的 a）→ 模糊（写回本级 a）。
        // 合成 pass 读的就是最后这轮纵向模糊的结果。
        record_fullscreen(
            &mut encoder,
            &pipelines.bright,
            &bright_bind,
            &chain.a[0],
            "xgc-bloom-bright-pass",
        );
        for level in 0..BLOOM_LEVELS - 1 {
            record_fullscreen(
                &mut encoder,
                &pipelines.down,
                &chain.down[level],
                &chain.a[level + 1],
                "xgc-bloom-down-pass",
            );
        }
        for level in 0..BLOOM_LEVELS {
            record_fullscreen(
                &mut encoder,
                &pipelines.blur,
                &chain.blur_h[level],
                &chain.b[level],
                "xgc-bloom-blur-h-pass",
            );
            record_fullscreen(
                &mut encoder,
                &pipelines.blur,
                &chain.blur_v[level],
                &chain.a[level],
                "xgc-bloom-blur-v-pass",
            );
        }
        context.queue().submit(Some(encoder.finish()));
        BLOOM_PASSES
    }

    /// 三级光晕的最终结果；Bloom 关着（还没跑过链）时返回 `None`。
    pub(crate) fn views(&self) -> Option<[&wgpu::TextureView; BLOOM_LEVELS]> {
        let chain = self.chain.as_ref()?;
        Some([&chain.a[0], &chain.a[1], &chain.a[2]])
    }

    /// 关掉光晕后丢掉中间链：旧端同样在 `intensity == 0` 时释放链。
    ///
    /// 留着链就白占 6 张 `Rgba16Float`（1280×720 约 11 MiB，4K 约 94 MiB），
    /// 而且关闭态还会把上一轮的残留视图绑进 tone-map。管线与采样器留着，
    /// 重新打开时不必重编译着色器。
    pub(crate) fn release(&mut self) {
        self.chain = None;
    }

    /// 当前链的目标尺寸；只给测试断言"源尺寸变了真的重建"。
    #[cfg(test)]
    pub(crate) fn chain_size(&self) -> Option<(u32, u32)> {
        self.chain.as_ref().map(|chain| (chain.width, chain.height))
    }
}

#[cfg(test)]
mod tests;
