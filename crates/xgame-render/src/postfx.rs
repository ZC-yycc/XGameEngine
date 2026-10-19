//! 后处理链：HDR 渲染路径 + 色调映射。
//!
//! # 为什么场景 pass 必须先进 HDR 中间标
//!
//! LDR 目标的写入是**钳制**的：8 位 sRGB 纹理存不下 4.0 的高光，超出的部分在
//! 写下去的那一刻就被裁成纯白 —— 高光细节不是"丢了"，而是**从来没被看见过**。
//! 色调映射（tone mapping）的全部意义就是在这次裁剪**之前**拿到真实亮度，
//! 把 [0, ∞) 压缩进 [0, 1]。所以场景 pass 渲进 `Rgba16Float`（线性光、可超过
//! 1.0），再有一条全屏后处理 pass 把它映射到显示目标。
//!
//! ```text
//!  场景 pass（HDR, MSAA）──resolve──▶ HDR 单采样 ──后处理 pass──▶ LDR 显示目标
//!                                     （色调映射 + 曝光）
//! ```
//!
//! # 为什么不是通用 RenderGraph
//!
//! 通用帧图（自动推导资源依赖、内存别名、异步计算调度）是给"十几个 pass 互相
//! 借纹理"的规模准备的。本引擎当前的后处理链是**线性的一串**：色调映射今天
//! 就一步，Bloom 也只是一串固定顺序。现在抽一层图，换来的只是抽象税。
//! 等出现真正带依赖分叉的 pass（SSAO 要用深度、TAA 要历史缓冲、CSM 要重投影）
//! 再抽图 —— 到时候是**有信息**地抽，而不是凭想象。
//!
//! # Bloom：亮部溢出的柔光
//!
//! ```text
//!  HDR ──亮部提取(软膝)+½降采样──▶ a0 ──降采样──▶ a1 ──降采样──▶ a2
//!                                   │            │            │
//!                                   ├─模糊(横/纵)─┤（a、b 两块纹理乒乓，
//!                                   ▼            ▼   同纹理不能既当输入又当输出）
//!                             a0,a1,a2（三级模糊结果）
//!  HDR ──┬──▶ 曝光 ──▶ + Σ(aᵢ×wᵢ)×strength ──▶ ACES ──▶ LDR
//!        └─────────────────────────────────┘（合成在同一趟全屏 pass 里）
//! ```
//!
//! - **阈值在曝光之后判**：摄影语义上"多亮算亮"取决于曝光后的画面值
//!   （threshold = 1.0 就是显示白点）。所以亮部提取要乘曝光，合成时也加在
//!   曝光后的颜色上。
//! - **软膝（soft knee）**：阈值附近用一段二次曲线平滑起步（Unity URP 的
//!   口径），不软膝的话亮斑边缘是一圈"刀切"的硬边，比不做还难看。
//! - **按最大通道判亮度**：纯红的高光不该因为"平均亮度低"不发光的 —— 彩灯
//!   的光晕是彩色的，这是所有主流实现的一致选择。
//! - **默认关闭（`BloomConfig::intensity == 0`）**：Bloom 是风格参数而不是
//!   正确性参数（对比：色调映射默认开，因为它回答"高光怎么落进显示区间"这个
//!   必须回答的问题）。关着时渲染器**整段跳过** —— 不建纹理、不记录 pass，
//!   是真正的零成本，而不是"跑一遍然后乘 0"。
//! - **三级 mip（½、¼、⅛）+ 可分离二项高斯**：同一套 9 抽头核在三级分辨率上
//!   跑，叠出由近及远的柔和光晕；合成权重 `[0.5, 0.3, 0.2]`（和为 1，
//!   与强度相乘后是"光晕总能量"的线性度量）。
//!
//! # 色调映射曲线的选择（ACES）
//!
//! 用的是 Stephen Hill 的 ACES 拟合（RRT + ODT 的有理函数拟合），三.js / Unity
//! 都在用同一份近似：先把亮度送进 ACEScg（AP1）做曲线，再回到 sRGB 原色域。
//! 与"直接对 RGB 套一条曲线"的朴素做法相比，它最大的实际收益是**过曝处的色相
//! 漂移小**：纯红的高光烧出来仍是红偏白，不会变成橙黄。
//!
//! 曲线前把亮度除以 0.6：RRT+ODT 拟合把 0.18 中灰映到 ~0.10（整幅暗一档）。
//! 本引擎的场景亮度是按"白面 ≈ 1.0 线性"标定的（PBR 的 π 校准，见
//! `scene_pipeline` 的 π 一节），除以 0.6 把中灰拉回 ~0.21，**整体亮度基线
//! 不因接入色调映射而变化** —— 用户看到的变化只发生在高光那一端（不再死白），
//! 中间调与暗部几乎不动。这与 PBR 当年的 π 校准是同一条原则。
//!
//! # `ToneMapMode::Off` 的定位
//!
//! 它不是"调试开关"，是一个**兼容模式**：曝光 + 钳制的输出与色调映射上线前的
//! 旧管线逐位一致（场景 shader 写线性值 → sRGB 目标硬件编码 → 硬件钳制）。
//! 旧项目迁移、像素回归对照都靠它。

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use xgame_core::{RenderConfig, ToneMapMode};

/// HDR 中间标的格式。
///
/// `Rgba16Float` 是 WebGPU 的**核心**（不需要任何 feature）格式里唯一同时支持
/// "浮点、可混合、可 MSAA、可线性过滤"的颜色目标：16 位浮点足够表示
/// [0, 65504] 的线性光（高光），存储带宽又只有 `Rgba32Float` 的一半。
/// 场景 pass 的 alpha 混合、MSAA resolve、采样器的线性过滤都依赖上述能力。
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

// ---------------------------------------------------------------------------
// HDR 渲染目标
// ---------------------------------------------------------------------------

/// 场景 pass 的一整套附件：HDR 颜色（单采样）+ MSAA HDR 颜色 + 深度。
///
/// 三家宿主（编辑器视口 / 独立游戏 / 无头截图）各有一份自己的重建策略
/// （编辑器要节流重建、游戏跟 resize、截图建一次），但"需要哪几张纹理、
/// 采样数怎么对"是同一件事 —— 收在这里，免得三份手写纹理描述各自漂移
/// （采样数不匹配的渲染通道会在提交时直接校验失败）。
pub struct HdrTargets {
    /// 单采样 HDR 颜色：MSAA resolve 的落点，也是后处理 pass 的输入。
    pub color: wgpu::Texture,
    pub color_view: wgpu::TextureView,
    /// MSAA 颜色附件（`sample_count > 1` 时存在）。场景 pass 渲进它，
    /// 再 resolve 到 `color`。
    pub msaa: Option<wgpu::Texture>,
    pub msaa_view: Option<wgpu::TextureView>,
    /// 深度附件。采样数必须与颜色附件一致。
    pub depth: wgpu::Texture,
    pub depth_view: wgpu::TextureView,
    pub size: (u32, u32),
    pub sample_count: u32,
}

impl HdrTargets {
    /// 按给定尺寸与采样数建一整套目标。`label_prefix` 只影响调试名
    /// （渲染文档抓帧时能一眼看出是哪家宿主的纹理）。
    pub fn new(
        device: &wgpu::Device,
        label_prefix: &str,
        width: u32,
        height: u32,
        sample_count: u32,
    ) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let sample_count = sample_count.max(1);
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("{label_prefix}_hdr_color")),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            // RENDER_ATTACHMENT：MSAA 的 resolve 目标；TEXTURE_BINDING：
            // 后处理 pass 的输入。
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());

        let msaa = (sample_count > 1).then(|| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(&format!("{label_prefix}_hdr_msaa_color")),
                size,
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FORMAT,
                // 只当渲染目标：resolve 之后才是 `color` 那张，这张不被采样。
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
        });
        let msaa_view = msaa
            .as_ref()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));

        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("{label_prefix}_hdr_depth")),
            size,
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            color,
            color_view,
            msaa,
            msaa_view,
            depth,
            depth_view,
            size: (width, height),
            sample_count,
        }
    }
}

// ---------------------------------------------------------------------------
// 后处理 pass
// ---------------------------------------------------------------------------

/// 后处理（合成）pass 的 uniform（必须与 WGSL 逐字节对齐）。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct PostParams {
    exposure: f32,
    /// 0 = 关闭（只曝光 + 钳制），1 = ACES。
    tone_map: u32,
    /// 1 = 目标不是 sRGB 格式，着色器里手动做 sRGB 编码。
    manual_srgb: u32,
    /// 光晕强度（0 = 不加；着色器按它分支，不去读各级光晕纹理）。
    bloom_intensity: f32,
    /// 三级 mip 的合成权重。
    ///
    /// 写成三个标量而不是 `[f32; 3]`：WGSL 里 uniform 数组的元素步长有 16 字节
    /// 对齐要求，Rust 端 `[f32; 3]` 是紧凑的 12 字节 —— 两边"看起来一样"实则
    /// 错位，是这类 uniform 最常见的一种静默 bug。三个标量逐字节可控。
    bloom_w0: f32,
    bloom_w1: f32,
    bloom_w2: f32,
    _pad: u32,
}

/// Bloom 各 pass 的 uniform（必须与 WGSL 逐字节对齐）。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct BloomParams {
    exposure: f32,
    threshold: f32,
    knee: f32,
    _pad: f32,
    /// 源纹理的 texel 尺寸（模糊步长的基数；其余 pass 用不到）。
    texel: [f32; 2],
    /// 模糊方向：`(1,0)` 横向 / `(0,1)` 纵向（其余 pass 用不到）。
    direction: [f32; 2],
}

/// Bloom 的 mip 级数（½、¼、⅛ 三级）。
const BLOOM_LEVELS: usize = 3;
/// 三级 mip 在合成时的权重（和为 1）：小 mip 更模糊、覆盖更广，权重更低。
const BLOOM_WEIGHTS: [f32; BLOOM_LEVELS] = [0.5, 0.3, 0.2];

/// 目标格式是否需要着色器手动做 sRGB 编码。
///
/// `Rgba8UnormSrgb` 这类格式由**硬件**在写入时编码（着色器输出线性值）；
/// 而非 sRGB 格式（`Bgra8Unorm` 之类）的数值会被原样当成"已编码的显示值"，
/// 线性值直接写进去画面会明显偏暗。窗口交换链的格式由系统挑，两者都可能
/// 出现 —— 所以这个判断必须存在，而不是赌交换链一定是 sRGB。
pub(crate) fn needs_manual_srgb(format: wgpu::TextureFormat) -> bool {
    !format.is_srgb()
}

/// 合成绑定组的缓存键：源视图代计数、源尺寸、是否走 Bloom 链。
///
/// 单独起个名字：三元组裸写在字段类型上过不了 clippy 的类型复杂度阈值，
/// 读代码的人也难一眼分清谁是谁。
type CompositeKey = (u64, (u32, u32), bool);

/// 全屏后处理管线：源（HDR）→ 曝光 →（合成 Bloom 光晕）→ 色调映射 → 目标（LDR）。
///
/// 生命周期与宿主的渲染目标绑定：目标重建（尺寸变化）时绑定组要跟着换，
/// 用 `source_gen`（宿主维护的代计数）判断 —— 见 [`Self::render`]。
pub struct PostFx {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    /// 合成绑定组的缓存：键 =（源代, 源尺寸, 是否走 Bloom 链）。
    ///
    /// 每帧新建绑定组是白给的分配（wgpu 内部要建表项 + 校验），而这三个量只在
    /// 目标重建 / Bloom 开关切换 / 链重建时才变。用宿主的代计数而不是指针判等：
    /// `TextureView` 没有稳定的身份，指针在释放后可能被复用（判定"没变"其实是
    /// 变了的陈旧视图，表现是画面停留在旧尺寸的最后一帧上）。
    /// 尺寸必须进键：绑定组引用 Bloom 各级的纹理视图，链按尺寸重建时源代可能
    /// 纹丝不动（截图器恒定 0 代），漏了尺寸 = 永远复用旧链的视图。
    bind_cache: Option<(CompositeKey, wgpu::BindGroup)>,
    /// Bloom 三个 pass 的管线（亮部提取 / 降采样 / 可分离模糊）。
    bloom_pipelines: BloomPipelines,
    /// Bloom 的 mip 链。**惰性建**：`intensity == 0` 时保持 `None`（不建纹理、
    /// 不记录 pass —— 真零成本）；一旦关闭就把已建的链丢掉（不养这份显存，
    /// 开关是 Inspector 里的手动操作，重建一次的代价无感）。
    bloom: Option<BloomChain>,
    /// 目标是否为 sRGB 格式（决定写 uniform 里的手动编码标志）。
    manual_srgb: bool,
}

impl PostFx {
    /// `output_format` 是**最终显示目标**的格式（编辑器视口纹理 / 交换链 /
    /// 截图纹理）。管线只依赖这一件事 —— MSAA 与尺寸都与它无关（后处理是
    /// 全屏单采样 pass）。
    pub fn new(device: &wgpu::Device, output_format: wgpu::TextureFormat) -> Self {
        // 纹理槽位（0 = 源 HDR，3..5 = Bloom 三级 mip）是同一形态：D2、可过滤、
        // 非多重采样。收成一个闭包 —— 六个手写条目迟早错一个绑定号，而绑定号
        // 错位在跑起来之前看不出来（着色器读到的是别的纹理，画面静默错掉）。
        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("postfx_bgl"),
            entries: &[
                tex_entry(0),
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
                tex_entry(3),
                tex_entry(4),
                tex_entry(5),
            ],
        });

        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("postfx_params"),
            contents: bytemuck::bytes_of(&PostParams {
                exposure: 1.0,
                tone_map: 1,
                manual_srgb: 0,
                bloom_intensity: 0.0,
                bloom_w0: BLOOM_WEIGHTS[0],
                bloom_w1: BLOOM_WEIGHTS[1],
                bloom_w2: BLOOM_WEIGHTS[2],
                _pad: 0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("postfx_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            // 后处理是 1:1 全屏采样，过滤模式在正常路径下"用不到"；
            // 但窗口尺寸与目标尺寸可能差一像素（缩放节流期间的视口），
            // 线性过滤比最近邻的观感好，也不贵。
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("postfx_layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("postfx_shader"),
            source: wgpu::ShaderSource::Wgsl(POSTFX_WGSL.into()),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("postfx_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                // 全屏三角形：位置由 vertex_index 现算，没有顶点缓冲。
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: output_format,
                    // 不混合：后处理 pass 的输出就是最终画面（它与显示目标
                    // 之间不存在"叠加"关系）。
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // 三角形在裁剪空间里是"超屏"的（-1..3），不做剔除。
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            bgl,
            sampler,
            uniform,
            bind_cache: None,
            bloom_pipelines: BloomPipelines::new(device),
            bloom: None,
            manual_srgb: needs_manual_srgb(output_format),
        }
    }

    /// 记录一趟：源 HDR 视图 → 曝光 →（Bloom 光晕）→ 色调映射 → 目标视图。
    ///
    /// `source_gen` 是宿主对"当前源视图是第几代目标"的计数：目标重建时 +1。
    /// `source_size` 是源的像素尺寸：Bloom 链按它定各级分辨率，尺寸变了就重建
    /// （截图器这类恒定 0 代的宿主只有尺寸这一个重建判据）。绑定组按二者与
    /// Bloom 开关缓存（见 `bind_cache` 的说明）。
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        source_gen: u64,
        source_size: (u32, u32),
        dest: &wgpu::TextureView,
        settings: RenderConfig,
    ) {
        let settings = settings.sanitized();
        // `intensity == 0` 是"关闭"的唯一形态（见 `BloomConfig` 的文档）。
        let bloom_on = settings.bloom.intensity > 0.0;
        if !bloom_on {
            // 关着就不养这份显存；再开时重建一次链（开关是手动操作，无感）。
            self.bloom = None;
        }
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&PostParams {
                exposure: settings.exposure,
                tone_map: match settings.tone_map {
                    ToneMapMode::Aces => 1,
                    ToneMapMode::Off => 0,
                },
                manual_srgb: u32::from(self.manual_srgb),
                bloom_intensity: if bloom_on { settings.bloom.intensity } else { 0.0 },
                bloom_w0: BLOOM_WEIGHTS[0],
                bloom_w1: BLOOM_WEIGHTS[1],
                bloom_w2: BLOOM_WEIGHTS[2],
                _pad: 0,
            }),
        );

        if bloom_on {
            if self.bloom.as_ref().map(|chain| chain.size) != Some(source_size) {
                self.bloom = Some(BloomChain::new(
                    device,
                    source_size,
                    &self.bloom_pipelines,
                    &self.sampler,
                ));
            }
            let chain = self.bloom.as_mut().expect("刚写入过链");
            chain.write_bright_params(queue, settings);
            // Bloom 的 pass 必须记录在合成 pass **之前**：编码器内的 pass 按
            // 记录顺序执行，链的结果当帧就能被合成读到（同一队列上有隐式的
            // 使用屏障，不需要手动同步）。
            chain.record(
                device,
                encoder,
                &self.bloom_pipelines,
                &self.sampler,
                source,
                source_gen,
            );
        }

        let key = (source_gen, source_size, bloom_on);
        if self.bind_cache.as_ref().map(|(k, _)| *k) != Some(key) {
            // Bloom 关着时 mip 槽位用源视图占位：布局里有的槽位必须填
            // （绑定组要求逐项给资源），着色器按 `bloom_intensity` 分支
            // 不去采样它们。
            let mips: [&wgpu::TextureView; BLOOM_LEVELS] = match self.bloom.as_ref() {
                Some(chain) => [&chain.a_views[0], &chain.a_views[1], &chain.a_views[2]],
                None => [source, source, source],
            };
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("postfx_bg"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(mips[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(mips[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(mips[2]),
                    },
                ],
            });
            self.bind_cache = Some((key, bind_group));
        }
        let bind_group = &self.bind_cache.as_ref().expect("刚写入过缓存").1;

        record_fullscreen_pass(encoder, &self.pipeline, bind_group, dest, "postfx_pass");
    }
}

// ---------------------------------------------------------------------------
// Bloom：三个 pass 的管线与 mip 链
// ---------------------------------------------------------------------------

/// 记录一趟全屏三角形 pass：清黑 + 画一个覆盖整屏的三角形。
///
/// 后处理与 Bloom 的每一趟都是这一形态，收成一个函数 —— 十来个手写的通道
/// 描述迟早有一处写错 `load`（要么校验失败，要么更糟：静默叠加到旧内容上）。
fn record_fullscreen_pass(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    target: &wgpu::TextureView,
    label: &str,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            ops: wgpu::Operations {
                // 全屏三角形铺满整个目标，load 本来无关紧要；显式清黑是
                // 为了让"没有 GPU 覆盖到的地方"有确定值（某些后端的视口
                // 裁剪边界上会留一像素）。
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}

/// 第 `level` 级 mip 的分辨率：½、¼、⅛，且不小于 1×1（小视口下不能出现
/// 0 尺寸纹理 —— wgpu 会直接校验失败）。
fn level_size(size: (u32, u32), level: usize) -> (u32, u32) {
    (
        (size.0.max(1) >> (level + 1)).max(1),
        (size.1.max(1) >> (level + 1)).max(1),
    )
}

/// Bloom 三个 pass 的管线（共用一条布局：单纹理 + 采样器 + uniform）。
///
/// 三个 pass 都是"全屏三角形 + 极少算术"，管线本身没有差异化状态，拆成三条
/// 只是为了入口不同。目标固定 `HDR_FORMAT`：Bloom 全程在线性 HDR 空间里做
/// （阈值、权重都是线性光量，混进色调映射后的显示值会随曲线漂移）。
struct BloomPipelines {
    bgl: wgpu::BindGroupLayout,
    bright: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
}

impl BloomPipelines {
    fn new(device: &wgpu::Device) -> Self {
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom_bgl"),
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
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bloom_layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bloom_shader"),
            source: wgpu::ShaderSource::Wgsl(BLOOM_WGSL.into()),
        });
        let pipeline = |label: &str, fs: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        // 不混合：每趟的输出都是"新的中间结果"，没有叠加语义
                        // （`LoadOp::Clear` 已经把旧内容清掉了）。
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // 三角形在裁剪空间里是"超屏"的（-1..3），不做剔除。
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        Self {
            bgl,
            bright: pipeline("bloom_bright_pipeline", "fs_bright"),
            down: pipeline("bloom_down_pipeline", "fs_down"),
            blur: pipeline("bloom_blur_pipeline", "fs_blur"),
        }
    }
}

/// Bloom 的 mip 链：3 级分辨率（½、¼、⅛）各一对 a/b 纹理 + 预建的绑定组。
///
/// 为什么是"一对纹理乒乓"：同一张纹理不能既当 pass 的输入又当输出（wgpu 在
/// 提交时校验）。横向模糊把 a 写进 b，纵向再把 b 写回 a —— 每级两张就够。
/// 纹理一律 `RENDER_ATTACHMENT | TEXTURE_BINDING`；每趟 pass 都用
/// `LoadOp::Clear` 且全屏三角形铺满目标，内容每帧被完整覆写，不存在
/// "采样到未初始化内容"的窗口。
///
/// 纹理本体在视图建好后即放手：wgpu 的视图持有纹理的 `Arc`（绑定组同理），
/// 句柄不是生命周期的锚点。
struct BloomChain {
    /// 建链时的源尺寸（`PostFx::render` 的重建判据）。
    size: (u32, u32),
    /// 每级的模糊结果 —— 合成 pass 读的就是这三个视图。
    a_views: [wgpu::TextureView; BLOOM_LEVELS],
    /// 横向模糊的落点（纵向模糊再写回 `a_views`）。
    b_views: [wgpu::TextureView; BLOOM_LEVELS],
    /// 亮部提取的 uniform：曝光 / 阈值 / 软膝随设置每帧变，所以这条可写。
    bright_uniform: wgpu::Buffer,
    /// 亮部提取的绑定组缓存（键 = 源代）：源视图只在目标重建时换。
    bright_bg: Option<(u64, wgpu::BindGroup)>,
    /// 降采样 pass 的绑定组（a0→a1、a1→a2，恒定不缓存）。
    down_bgs: [wgpu::BindGroup; BLOOM_LEVELS - 1],
    /// 每级横向（a→b）模糊的绑定组。
    blur_h_bgs: [wgpu::BindGroup; BLOOM_LEVELS],
    /// 每级纵向（b→a）模糊的绑定组。
    blur_v_bgs: [wgpu::BindGroup; BLOOM_LEVELS],
}

impl BloomChain {
    fn new(
        device: &wgpu::Device,
        size: (u32, u32),
        pipelines: &BloomPipelines,
        sampler: &wgpu::Sampler,
    ) -> Self {
        let size = (size.0.max(1), size.1.max(1));
        let make_view = |label: &str, (width, height): (u32, u32)| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
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
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            texture.create_view(&wgpu::TextureViewDescriptor::default())
        };
        let a_views: [wgpu::TextureView; BLOOM_LEVELS] = std::array::from_fn(|level| {
            make_view(&format!("bloom_a{level}"), level_size(size, level))
        });
        let b_views: [wgpu::TextureView; BLOOM_LEVELS] = std::array::from_fn(|level| {
            make_view(&format!("bloom_b{level}"), level_size(size, level))
        });

        let bright_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bloom_bright_params"),
            contents: bytemuck::bytes_of(&BloomParams {
                exposure: 1.0,
                threshold: 1.0,
                knee: 0.6,
                _pad: 0.0,
                texel: [0.0; 2],
                direction: [0.0; 2],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        // 降采样 pass 不读 uniform，但布局是三个 pass 共用的 —— 绑一条零值
        // 缓冲比自己再维护一套布局 + 一个 WGSL 类型便宜。
        let plain_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bloom_plain_params"),
            contents: bytemuck::bytes_of(&BloomParams::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        // 模糊的 texel 与方向建链时定死（静态缓冲；未用到的字段填中性值）。
        let blur_uniforms = |direction: [f32; 2]| -> [wgpu::Buffer; BLOOM_LEVELS] {
            std::array::from_fn(|level| {
                let (width, height) = level_size(size, level);
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("bloom_blur_params"),
                    contents: bytemuck::bytes_of(&BloomParams {
                        exposure: 1.0,
                        threshold: 1.0,
                        knee: 0.0,
                        _pad: 0.0,
                        texel: [1.0 / width as f32, 1.0 / height as f32],
                        direction,
                    }),
                    usage: wgpu::BufferUsages::UNIFORM,
                })
            })
        };
        let blur_h_uniforms = blur_uniforms([1.0, 0.0]);
        let blur_v_uniforms = blur_uniforms([0.0, 1.0]);

        let make_bg = |label: &str, view: &wgpu::TextureView, uniform: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &pipelines.bgl,
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
                        resource: uniform.as_entire_binding(),
                    },
                ],
            })
        };

        // 降采样、模糊的绑定组建链时一次性建好（源与目标都在链内、不会换）；
        // 亮部提取的源是外部的，按代缓存。
        let down_bgs =
            std::array::from_fn(|level| make_bg("bloom_down_bg", &a_views[level], &plain_uniform));
        let blur_h_bgs = std::array::from_fn(|level| {
            make_bg("bloom_blur_h_bg", &a_views[level], &blur_h_uniforms[level])
        });
        let blur_v_bgs = std::array::from_fn(|level| {
            make_bg("bloom_blur_v_bg", &b_views[level], &blur_v_uniforms[level])
        });

        Self {
            size,
            a_views,
            b_views,
            bright_uniform,
            bright_bg: None,
            down_bgs,
            blur_h_bgs,
            blur_v_bgs,
        }
    }

    /// 写亮部提取的 uniform（曝光 / 阈值 / 软膝 —— 三者都参与"多亮算亮"）。
    fn write_bright_params(&self, queue: &wgpu::Queue, settings: RenderConfig) {
        queue.write_buffer(
            &self.bright_uniform,
            0,
            bytemuck::bytes_of(&BloomParams {
                exposure: settings.exposure,
                threshold: settings.bloom.threshold,
                knee: settings.bloom.knee,
                _pad: 0.0,
                texel: [0.0; 2],
                direction: [0.0; 2],
            }),
        );
    }

    /// 记录一整条链：亮部提取（源 → a0）→ 逐级降采样（a0→a1→a2）→
    /// 逐级横/纵模糊（a↔b）。调用方保证 `bright_uniform` 已按当前设置写好。
    fn record(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &BloomPipelines,
        sampler: &wgpu::Sampler,
        source: &wgpu::TextureView,
        source_gen: u64,
    ) {
        if self.bright_bg.as_ref().map(|(gen, _)| *gen) != Some(source_gen) {
            self.bright_bg = Some((
                source_gen,
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("bloom_bright_bg"),
                    layout: &pipelines.bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(source),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: self.bright_uniform.as_entire_binding(),
                        },
                    ],
                }),
            ));
        }
        let bright_bg = &self.bright_bg.as_ref().expect("刚写入过缓存").1;

        // 顺序固定：亮部提取 → 降采样（写高一级的 a）→ 模糊（写回本级 a）。
        // 合成 pass 读的就是最后这轮模糊的结果。
        record_fullscreen_pass(
            encoder,
            &pipelines.bright,
            bright_bg,
            &self.a_views[0],
            "bloom_bright_pass",
        );
        for level in 0..BLOOM_LEVELS - 1 {
            record_fullscreen_pass(
                encoder,
                &pipelines.down,
                &self.down_bgs[level],
                &self.a_views[level + 1],
                "bloom_down_pass",
            );
        }
        for level in 0..BLOOM_LEVELS {
            record_fullscreen_pass(
                encoder,
                &pipelines.blur,
                &self.blur_h_bgs[level],
                &self.b_views[level],
                "bloom_blur_h_pass",
            );
            record_fullscreen_pass(
                encoder,
                &pipelines.blur,
                &self.blur_v_bgs[level],
                &self.a_views[level],
                "bloom_blur_v_pass",
            );
        }
    }
}

/// 后处理着色器。
///
/// 与 [`super::postfx`] 模块文档的"曲线选择"一节配套：ACES 拟合的矩阵与
/// 有理函数是 Stephen Hill 版（三.js `ACESFilmicToneMapping` /
/// Unity ACES approximation 同一份数值），没有魔改。
const POSTFX_WGSL: &str = r#"
struct PostParams {
    exposure: f32,
    tone_map: u32,          // 0 = 关闭（只曝光 + 钳制），1 = ACES
    manual_srgb: u32,       // 1 = 目标不是 sRGB 格式，着色器里手动编码
    bloom_intensity: f32,   // 0 = 不加光晕（分支跳过，不去采样各级纹理）
    bloom_w0: f32,          // 三级光晕的合成权重
    bloom_w1: f32,
    bloom_w2: f32,
    _pad: u32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_smp: sampler;
@group(0) @binding(2) var<uniform> params: PostParams;
// Bloom 三级光晕（½ / ¼ / ⅛）。Bloom 关闭时绑定源纹理占位 ——
// 分支不进就不采样，占位内容无关紧要。
@group(0) @binding(3) var bloom0_tex: texture_2d<f32>;
@group(0) @binding(4) var bloom1_tex: texture_2d<f32>;
@group(0) @binding(5) var bloom2_tex: texture_2d<f32>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    // 全屏三角形（比两个三角形少一条对角线接缝，也少一次重复着色）。
    var xy = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let p = xy[index];
    var out: VsOut;
    out.clip = vec4<f32>(p, 0.0, 1.0);
    // 纹理坐标 v 朝下、NDC 的 y 朝上 —— 这里翻一次，别把画面上下颠倒。
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

// ACES 拟合（RRT + ODT 的有理函数近似）的输入 / 输出色域转换：
// sRGB -> AP1（ACEScg）做曲线的形状，再 AP1 -> sRGB 回来。
// 列主序（WGSL 的 mat3x3 构造即列向量），与参考实现的数值逐字一致。
const ACES_INPUT_MAT = mat3x3<f32>(
    vec3<f32>(0.59719, 0.07600, 0.02840),
    vec3<f32>(0.35458, 0.90834, 0.13383),
    vec3<f32>(0.04823, 0.01566, 0.83777),
);
const ACES_OUTPUT_MAT = mat3x3<f32>(
    vec3<f32>( 1.60475, -0.10208, -0.00327),
    vec3<f32>(-0.53108,  1.10813, -0.07276),
    vec3<f32>(-0.07367, -0.00605,  1.07602),
);

fn rrt_and_odt_fit(v: vec3<f32>) -> vec3<f32> {
    let a = v * (v + vec3<f32>(0.0245786)) - vec3<f32>(0.000090537);
    let b = v * (vec3<f32>(0.983729) * v + vec3<f32>(0.4329510)) + vec3<f32>(0.238081);
    return a / b;
}

// 线性 -> sRGB 传输函数的编码方向（用于非 sRGB 目标的手动编码）。
fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(lo, hi, c > vec3<f32>(0.0031308));
}

@fragment
fn fs_main(input: VsOut) -> @location(0) vec4<f32> {
    // max 0：浮点目标上单个 NaN 会污染整帧（GGX 的防护见 MIN_ROUGHNESS），
    // 这里再兜一层 —— 后处理是最后一道关，负值进曲线会出负数。
    var c = max(textureSample(src_tex, src_smp, input.uv).rgb, vec3<f32>(0.0)) * params.exposure;

    // Bloom 加在曝光后、曲线前：光晕也要过色调映射（不然高阈值时合成出来
    // 的光晕先在 LDR 端被裁出硬边）。条件来自 uniform，同一趟 pass 内所有
    // 像素走同一分支 —— 纹理采样处在一致性控制流里。
    if (params.bloom_intensity > 0.0) {
        let g0 = textureSample(bloom0_tex, src_smp, input.uv).rgb;
        let g1 = textureSample(bloom1_tex, src_smp, input.uv).rgb;
        let g2 = textureSample(bloom2_tex, src_smp, input.uv).rgb;
        c += (g0 * params.bloom_w0 + g1 * params.bloom_w1 + g2 * params.bloom_w2)
            * params.bloom_intensity;
    }

    if (params.tone_map == 1u) {
        // 除以 0.6 的原因见模块文档"色调映射曲线的选择"一节：把中灰从
        // ~0.10 拉回 ~0.21，保住本引擎"白面 ≈ 1.0"的亮度基线。
        c = c / 0.6;
        c = ACES_INPUT_MAT * c;
        c = rrt_and_odt_fit(c);
        c = ACES_OUTPUT_MAT * c;
    }

    // 两条路径都要钳制：Off 模式的高光是"裁剪"语义（与旧管线一致），
    // ACES 的输出理论上已在 [0,1]，但拟合在极亮处可能冒出去一点。
    c = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    if (params.manual_srgb == 1u) {
        c = srgb_encode(c);
    }
    return vec4<f32>(c, 1.0);
}
"#;

/// Bloom 三趟 pass 的着色器：亮部提取（软膝 + ½ 降采样）、降采样、可分离模糊。
///
/// 三个 pass 共用 [`BloomParams`] 一条 uniform：亮部提取读曝光/阈值/软膝，
/// 模糊读 texel/direction —— 用不到的字段填中性值，省得维护三个类型。
const BLOOM_WGSL: &str = r#"
struct BloomParams {
    exposure: f32,
    threshold: f32,
    knee: f32,
    _pad: f32,
    texel: vec2<f32>,      // 源纹理的 texel 尺寸（模糊步长）
    direction: vec2<f32>,  // 模糊方向：(1,0) 横向 / (0,1) 纵向
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_smp: sampler;
@group(0) @binding(2) var<uniform> params: BloomParams;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    var xy = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let p = xy[index];
    var out: VsOut;
    out.clip = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

// 亮部提取：曝光 → 按最大通道判亮度 → 软膝曲线 → 原色乘"提取比例"。
//
// 权重是 contribution / brightness（有上界 1）而不是 contribution 本身：
// 软膝区里像素按比例"淡入"光晕，过了膝区后全额参与 —— 这样光晕不会比
// 源像素还亮。（Unity URP 的 Bloom Prefilter 就是这个式子。）
//
// 按最大通道判亮度：纯红的高光不该因为"平均亮度低"不发光，彩灯的光晕
// 是彩色的，主流实现一致如此。
@fragment
fn fs_bright(input: VsOut) -> @location(0) vec4<f32> {
    let c = max(textureSample(src_tex, src_smp, input.uv).rgb, vec3<f32>(0.0)) * params.exposure;
    let brightness = max(c.r, max(c.g, c.b));
    // 软膝：贡献从 (threshold - knee) 处的 0 二次升到 (threshold + knee) 处的
    // knee，之后线性。knee = 0 时退化为硬阈值（分母 +1e-5 兜住除零）。
    var soft = clamp(brightness - params.threshold + params.knee, 0.0, 2.0 * params.knee);
    soft = soft * soft / (4.0 * params.knee + 0.00001);
    let contribution = max(soft, brightness - params.threshold);
    let weight = contribution / max(brightness, 0.00001);
    return vec4<f32>(c * weight, 1.0);
}

// 降采样：输出像素中心恰好落在源 2×2 texel 块的中点（½ 尺寸），一次双线性
// 采样就是一次 2×2 箱式平均 —— 比四次点采样便宜，也没有采样点接缝。
@fragment
fn fs_down(input: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src_tex, src_smp, input.uv);
}

// 可分离二项高斯：权重 [1,8,28,56,70,56,28,8,1]/256（一半点对称写出来）。
// 对 ½ 降采样后的纹理，等效 σ ≈ 2 texel/级；三级叠加得到由近及远的柔和光晕。
@fragment
fn fs_blur(input: VsOut) -> @location(0) vec4<f32> {
    let step = params.texel * params.direction;
    var sum = textureSample(src_tex, src_smp, input.uv).rgb * 0.2734375;        // 70/256
    sum += textureSample(src_tex, src_smp, input.uv + step).rgb * 0.21875;      // 56/256
    sum += textureSample(src_tex, src_smp, input.uv - step).rgb * 0.21875;
    sum += textureSample(src_tex, src_smp, input.uv + step * 2.0).rgb * 0.109375;  // 28/256
    sum += textureSample(src_tex, src_smp, input.uv - step * 2.0).rgb * 0.109375;
    sum += textureSample(src_tex, src_smp, input.uv + step * 3.0).rgb * 0.03125;   // 8/256
    sum += textureSample(src_tex, src_smp, input.uv - step * 3.0).rgb * 0.03125;
    sum += textureSample(src_tex, src_smp, input.uv + step * 4.0).rgb * 0.00390625;  // 1/256
    sum += textureSample(src_tex, src_smp, input.uv - step * 4.0).rgb * 0.00390625;
    return vec4<f32>(sum, 1.0);
}
"#;

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod cpu {
    //! 与 WGSL 同式的 CPU 实现。
    //!
    //! 存在的理由：GPU 像素测试要**对表** —— 断言"像素值 ≈ 曲线在输入亮度
    //! 处的输出"时，预期值不该是手抄的魔数（改曲线要人肉重算一遍九处），
    //! 而应由这份镜像现算。它同时是 WGSL 数值的回归网：如果哪天 WGSL 里的
    //! 矩阵敲错一位，GPU 测试会与镜像分道扬镳而红。
    //!
    //! 纪律：这份代码与 WGSL **必须逐式对应**，改一边就改另一边。

    use xgame_core::{RenderConfig, ToneMapMode};

    const ACES_INPUT_MAT: [[f32; 3]; 3] = [
        [0.59719, 0.07600, 0.02840],
        [0.35458, 0.90834, 0.13383],
        [0.04823, 0.01566, 0.83777],
    ];
    const ACES_OUTPUT_MAT: [[f32; 3]; 3] = [
        [1.60475, -0.10208, -0.00327],
        [-0.53108, 1.10813, -0.07276],
        [-0.07367, -0.00605, 1.07602],
    ];

    fn mat_mul(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|r| m[0][r] * v[0] + m[1][r] * v[1] + m[2][r] * v[2])
    }

    fn rrt_and_odt_fit(v: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| {
            let x = v[i];
            let a = x * (x + 0.024_578_6) - 0.000_090_537;
            let b = x * (0.983_729 * x + 0.432_951) + 0.238_081;
            a / b
        })
    }

    /// ACES 曲线（含 1/0.6 的亮度基准确认），与 WGSL 的 `fs_main` 中
    /// `tone_map == 1` 分支一致。返回**显示参考**的线性值（未做 sRGB 编码）。
    pub(crate) fn aces_tone_map(c: [f32; 3]) -> [f32; 3] {
        let v = c.map(|x| (x.max(0.0)) / 0.6);
        let v = mat_mul(&ACES_INPUT_MAT, v);
        let v = rrt_and_odt_fit(v);
        let v = mat_mul(&ACES_OUTPUT_MAT, v);
        v.map(|x| x.clamp(0.0, 1.0))
    }

    /// 与 WGSL `fs_main` 同一条完整路径（曝光 → 曲线 → 钳制）。
    pub(crate) fn tone_map(linear: [f32; 3], settings: RenderConfig) -> [f32; 3] {
        let c = linear.map(|x| (x * settings.exposure).max(0.0));
        match settings.tone_map {
            ToneMapMode::Aces => aces_tone_map(c),
            ToneMapMode::Off => c.map(|x| x.clamp(0.0, 1.0)),
        }
    }

    /// Bloom 亮部提取（与 WGSL `fs_bright` 逐式对应）。输入是**线性场景光**，
    /// 返回三级光晕链的源头（还没经过降采样/模糊 —— 掩模测试对的就是提取本身）。
    pub(crate) fn bloom_brightness(
        linear: [f32; 3],
        exposure: f32,
        threshold: f32,
        knee: f32,
    ) -> [f32; 3] {
        let c = linear.map(|x| (x * exposure).max(0.0));
        let brightness = c[0].max(c[1]).max(c[2]);
        let mut soft = (brightness - threshold + knee).clamp(0.0, 2.0 * knee);
        soft = soft * soft / (4.0 * knee + 0.000_01);
        let contribution = soft.max(brightness - threshold);
        let weight = contribution / brightness.max(0.000_01);
        c.map(|x| x * weight)
    }

    /// 与 WGSL `fs_main` 同一条完整合成路径（曝光 → 加光晕 → 曲线 → 钳制）。
    ///
    /// `bloom` 是三级光晕的线性值：GPU 上来自 blur 链的最终 a 级纹理；镜像
    /// 测试直接填"提取值"（模糊是保能量的加权平均，合成公式不看这一点）。
    pub(crate) fn composite(
        linear: [f32; 3],
        bloom: [[f32; 3]; super::BLOOM_LEVELS],
        settings: RenderConfig,
    ) -> [f32; 3] {
        let mut c = linear.map(|x| (x * settings.exposure).max(0.0));
        if settings.bloom.intensity > 0.0 {
            let glow: [f32; 3] = std::array::from_fn(|ch| {
                (bloom[0][ch] * super::BLOOM_WEIGHTS[0]
                    + bloom[1][ch] * super::BLOOM_WEIGHTS[1]
                    + bloom[2][ch] * super::BLOOM_WEIGHTS[2])
                    * settings.bloom.intensity
            });
            c = std::array::from_fn(|ch| c[ch] + glow[ch]);
        }
        match settings.tone_map {
            ToneMapMode::Aces => aces_tone_map(c),
            ToneMapMode::Off => c.map(|x| x.clamp(0.0, 1.0)),
        }
    }

    /// 线性 → sRGB 编码（[`ToneMapMode::Off`] 下显示目标的手动编码路径）。
    pub(crate) fn srgb_encode(c: [f32; 3]) -> [f32; 3] {
        c.map(|x| {
            if x > 0.003_130_8 {
                1.055 * x.powf(1.0 / 2.4) - 0.055
            } else {
                x * 12.92
            }
        })
    }

    /// 线性值 → 8 位 sRGB 字节（GPU 读回的 PNG 像素就是这么存的）。
    pub(crate) fn to_srgb_u8(c: [f32; 3]) -> [u8; 3] {
        srgb_encode(c)
            .map(|x| (x.clamp(0.0, 1.0) * 255.0).round().clamp(0.0, 255.0) as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::cpu::*;
    use super::*;

    /// 端点：黑进黑出、白不炸出 1（ACES 的输出空间是 [0,1]），
    /// 且 1.0 的输入映射到明显小于 1 的显示值 —— "不再死白"的量化形态。
    #[test]
    fn aces_maps_black_to_black_and_compresses_white() {
        let black = aces_tone_map([0.0, 0.0, 0.0]);
        assert_eq!(black, [0.0, 0.0, 0.0]);

        let white = aces_tone_map([1.0; 3]);
        assert!(white[0] < 0.95, "1.0 的输入必须被压到 1 以下：{white:?}");
        assert!(white[0] > 0.7, "1.0 也不该被压得太狠：{white:?}");
        // 三条通道都是灰（输入是灰，输出没有色偏）。
        assert!((white[0] - white[1]).abs() < 1e-3);
        assert!((white[1] - white[2]).abs() < 1e-3);
    }

    /// 曲线单调：亮度升，输出不可能降。这是"高光滚降"而不是"过曝翻转"
    /// 的本质 —— 违反它意味着高光越亮画面越暗。
    #[test]
    fn aces_is_monotonic() {
        let mut prev = -1.0f32;
        for i in 0..=320 {
            let x = i as f32 * 0.1; // 0 .. 32 线性
            let y = aces_tone_map([x; 3])[0];
            assert!(y >= prev - 1e-6, "曲线在 {x} 处回折：{y} < {prev}");
            prev = y;
        }
    }

    /// 高光滚降的**行为级**断言：4.0 与 16.0 的输入（旧管线里都是纯白，
    /// 255 分不出来）必须映射到两个不同的值，而且都不到顶。
    #[test]
    fn aces_rolls_off_highlights_without_clipping() {
        let four = to_srgb_u8(aces_tone_map([4.0; 3]));
        let sixteen = to_srgb_u8(aces_tone_map([16.0; 3]));
        assert!(four[0] < 254, "4.0 的高光不该被裁到纯白：{four:?}");
        assert!(sixteen[0] > four[0], "更亮的高光必须更亮：16 -> {sixteen:?}, 4 -> {four:?}");
        // 旧管线（Off）里两者都是 255 —— 这条测试在 ACES 缺席时必红。
        let off = RenderConfig {
            exposure: 1.0,
            tone_map: ToneMapMode::Off,
            ..RenderConfig::default()
        };
        assert_eq!(to_srgb_u8(tone_map([4.0; 3], off)), [255, 255, 255]);
        assert_eq!(to_srgb_u8(tone_map([16.0; 3], off)), [255, 255, 255]);
    }

    /// 曝光在曲线**之前**生效：曝光翻倍 = 把输入亮度翻倍，而不是把输出
    /// 亮度翻倍（后者超 1 会被钳掉，且不符合影像语义）。
    #[test]
    fn exposure_scales_before_the_curve() {
        let one = RenderConfig {
            exposure: 1.0,
            tone_map: ToneMapMode::Aces,
            ..RenderConfig::default()
        };
        let two = RenderConfig { exposure: 2.0, ..one };
        assert_eq!(tone_map([0.5; 3], two), aces_tone_map([1.0; 3]));
        assert_eq!(tone_map([0.5; 3], one), aces_tone_map([0.5; 3]));
        let brighter = to_srgb_u8(tone_map([0.3; 3], two))[0];
        let normal = to_srgb_u8(tone_map([0.3; 3], one))[0];
        assert!(brighter > normal, "曝光 2× 必须更亮：{brighter} vs {normal}");
    }

    /// `ToneMapMode::Off` = 旧管线：线性值钳制后显示（高光裁剪）。
    #[test]
    fn off_mode_matches_the_pre_tonemap_pipeline() {
        let off = RenderConfig {
            exposure: 1.0,
            tone_map: ToneMapMode::Off,
            ..RenderConfig::default()
        };
        assert_eq!(tone_map([0.25; 3], off), [0.25; 3]);
        assert_eq!(tone_map([9.0; 3], off), [1.0; 3], "超高光被裁到纯白");
        // 与 sRGB 编码串联后就是旧管线写在 sRGB 目标上的那个字节
        // （`to_srgb_u8` 自带编码，别在这里再手动编码一次）。
        assert_eq!(to_srgb_u8(tone_map([0.25; 3], off)), to_srgb_u8([0.25; 3]));
    }

    /// 手动 sRGB 编码与传输函数对表（0.5 线性 → 188 / 255）。
    #[test]
    fn manual_srgb_encode_matches_the_transfer_function() {
        assert_eq!(to_srgb_u8([0.0; 3]), [0, 0, 0]);
        assert_eq!(to_srgb_u8([1.0; 3]), [255, 255, 255]);
        assert_eq!(to_srgb_u8([0.5; 3]), [188, 188, 188]);
        // 线性段（≤ 0.0031308）走 12.92 的直线：编码结果就是 x × 12.92。
        let encoded = srgb_encode([0.002; 3])[0];
        assert!(
            (encoded - 0.002 * 12.92).abs() < 1e-6,
            "线性段的编码应当是 12.92 倍直线，收到 {encoded}"
        );
        // 阈值两侧连续（线性段与幂函数段在 0.0031308 处接得上）。
        let below = srgb_encode([0.003_130_8; 3])[0];
        let above = srgb_encode([0.003_131; 3])[0];
        assert!((above - below).abs() < 1e-4, "传输函数在阈值处必须连续");
    }

    /// 目标格式决定是否手动编码：sRGB 格式交给硬件，非 sRGB 格式自己来。
    #[test]
    fn manual_srgb_is_derived_from_the_target_format() {
        assert!(!needs_manual_srgb(wgpu::TextureFormat::Bgra8UnormSrgb));
        assert!(!needs_manual_srgb(wgpu::TextureFormat::Rgba8UnormSrgb));
        assert!(needs_manual_srgb(wgpu::TextureFormat::Bgra8Unorm));
        assert!(needs_manual_srgb(wgpu::TextureFormat::Rgba8Unorm));
    }

    /// 色调映射后的**对比度**必须还在中间调上：0.18 中灰（标准场景灰）
    /// 显示值应落在 110~150 之间 —— 这是"1/0.6 亮度基线"承诺的量化形态。
    /// 若哪天有人把 1/0.6 删了，中灰会掉到 ~92，这条会红。
    #[test]
    fn mid_gray_stays_mid_gray() {
        let g = to_srgb_u8(aces_tone_map([0.18; 3]))[0];
        assert!(
            (110..=150).contains(&g),
            "0.18 中灰的显示值应当仍在中间调（1/0.6 基线的意义所在），收到 {g}"
        );
    }

    // -----------------------------------------------------------------------
    // Bloom（亮部提取与合成）
    // -----------------------------------------------------------------------

    /// 膝下限之下（亮度 ≤ threshold - knee）提取必须是纯黑：不该有任何光晕。
    #[test]
    fn bloom_brightness_stays_black_below_the_knee_floor() {
        let (t, k) = (1.0, 0.5);
        assert_eq!(bloom_brightness([0.3; 3], 1.0, t, k), [0.0; 3]);
        // 正好在膝下限：二次曲线的 0 端，且"过曝项" b - t 是负数 —— 输出仍是 0。
        assert_eq!(bloom_brightness([t - k; 3], 1.0, t, k), [0.0; 3]);
    }

    /// 软膝的形状：膝区里提取量单调上升，下端起于 0、上端接上直线段，
    /// 且**阈值以下就已经开始淡入**（硬阈值在同样位置还是 0）—— 最后这条
    /// 是判别性的：软膝被删成硬阈值时它必红。
    #[test]
    fn bloom_brightness_ramps_smoothly_through_the_knee() {
        let (t, k) = (1.0, 0.5);

        let mut prev = 0.0f32;
        for i in 0..=80 {
            let b = t - k + i as f32 * 0.0125; // 扫过整段膝区
            let e = bloom_brightness([b; 3], 1.0, t, k)[0];
            assert!(e >= prev - 1e-6, "膝区提取量在 b={b} 处回折：{e} < {prev}");
            prev = e;
        }

        // 膝区下半段（阈值以下）软膝已在淡入，硬阈值口径（knee = 0）还是 0。
        let b = t - k * 0.5;
        assert!(
            bloom_brightness([b; 3], 1.0, t, k)[0] > 0.0,
            "阈值以下的软膝淡入段丢了（被改成硬阈值？）"
        );
        assert_eq!(bloom_brightness([b; 3], 1.0, t, 0.0), [0.0; 3]);

        // 膝区上端与直线段接得上（曲线在 t + k 处的值 = knee = 直线段的值）。
        let top = t + k;
        let inside = bloom_brightness([top - 1e-3; 3], 1.0, t, k)[0];
        let outside = bloom_brightness([top + 1e-3; 3], 1.0, t, k)[0];
        assert!(
            (outside - inside).abs() < 3e-3,
            "膝区上端与直线段的接缝过大：{inside} vs {outside}"
        );
    }

    /// knee = 0 退化为硬阈值：阈值下全黑，阈值上提取 `(b - t) / b` 的比例。
    #[test]
    fn bloom_brightness_knee_zero_is_a_hard_threshold() {
        assert_eq!(bloom_brightness([0.9; 3], 1.0, 1.0, 0.0), [0.0; 3]);
        // 灰度 2.0：比例 (2-1)/2 = 0.5，原色 2.0 × 0.5 = 1.0 = b - t。
        let e = bloom_brightness([2.0; 3], 1.0, 1.0, 0.0);
        assert!((e[0] - 1.0).abs() < 1e-6, "硬阈值提取应为 b - t：{e:?}");
    }

    /// 亮度按**最大通道**判：纯红的高光要发光，两个暗通道平均不出高光。
    #[test]
    fn bloom_brightness_selects_by_the_max_channel() {
        let e = bloom_brightness([2.0, 0.0, 0.0], 1.0, 1.0, 0.0);
        assert!((e[0] - 1.0).abs() < 1e-6, "红通道应当全额参与提取：{e:?}");
        assert_eq!(e[1], 0.0);
        assert_eq!(e[2], 0.0);
        // 绿蓝都 0.9（都低于阈值）时红通道是 0；"平均亮度"口径会在这里误发光。
        assert_eq!(bloom_brightness([0.0, 0.9, 0.9], 1.0, 1.0, 0.0), [0.0; 3]);
    }

    /// 曝光在提取**之前**生效：阈值判的是曝光后的画面值（摄影语义）。
    #[test]
    fn bloom_brightness_applies_exposure_first() {
        // 0.6 的灰在曝光 1 下低于阈值；曝光 2× 后 1.2 过阈值，提取 0.2。
        assert_eq!(bloom_brightness([0.6; 3], 1.0, 1.0, 0.0), [0.0; 3]);
        let e = bloom_brightness([0.6; 3], 2.0, 1.0, 0.0);
        assert!((e[0] - 0.2).abs() < 1e-6, "曝光后提取应为 1.2 - 1.0：{e:?}");
    }

    /// 三级合成权重必须归一化：否则"强度 1.0"的含义随级数漂移
    /// （每加一级 mip 画面整体变亮一次，是没人想要的那种"改进"）。
    #[test]
    fn bloom_weights_sum_to_one() {
        let sum: f32 = BLOOM_WEIGHTS.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6, "合成权重必须归一化：{sum}");
    }

    /// 合成是"曝光 → 加光晕 → 曲线"：光晕加在**曲线之前**，与对表值一致。
    #[test]
    fn composite_adds_glow_before_the_curve() {
        let settings = RenderConfig {
            bloom: xgame_core::BloomConfig {
                intensity: 1.0,
                ..Default::default()
            },
            ..RenderConfig::default()
        };
        let glow = [[0.4; 3], [0.2; 3], [0.1; 3]];
        let weighted =
            0.4 * BLOOM_WEIGHTS[0] + 0.2 * BLOOM_WEIGHTS[1] + 0.1 * BLOOM_WEIGHTS[2];
        let got = composite([0.1; 3], glow, settings);
        let want = aces_tone_map([0.1 + weighted; 3]);
        for ch in 0..3 {
            assert!(
                (got[ch] - want[ch]).abs() < 1e-6,
                "第 {ch} 通道：{got:?} vs 对表 {want:?}"
            );
        }
        // 有光晕一定比没光晕亮（曲线单调，光晕是正增益）。
        assert!(got[0] > tone_map([0.1; 3], settings)[0]);
    }

    /// `intensity = 0`（默认）时合成与不带光晕的路径**逐位一致** —— "关"是
    /// 整段跳过而不是"乘以 0"（乘以 0 在光晕是 NaN 时会污染整幅画面）。
    #[test]
    fn composite_with_bloom_off_matches_plain_tone_map() {
        let settings = RenderConfig::default();
        let glow = [[9.0; 3], [9.0; 3], [9.0; 3]];
        assert_eq!(
            composite([0.3; 3], glow, settings),
            tone_map([0.3; 3], settings)
        );
        assert_eq!(
            composite([3.0; 3], glow, settings),
            tone_map([3.0; 3], settings)
        );
    }
}
