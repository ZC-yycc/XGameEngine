//! TAA（Temporal Anti-Aliasing，时域抗锯齿）：用历史帧换更干净的边缘。
//!
//! # 为什么值得做
//!
//! MSAA 只在**几何边缘**上采样（一个像素内多取几个覆盖位置），对"着色器里高频
//! 变化的东西"——细金属丝、远处栏杆、纹理里的强对比条纹——完全无能为力：那些
//! 像素的边界不是三角形边界，硬件没有额外的采样点可去。FXAA 是事后拿邻域亮度
//! 猜边缘，猜错就把细节一起抹平。
//!
//! TAA 换了个方向：**每帧把相机投影抖动一个亚像素**，于是同一个像素在连续几帧里
//! 采样了不同的子像素位置，把它们按权重累加，等价于"这个像素有 8 个采样点"。
//! 它不需要更高的分辨率（也不像 SSAA 那样把像素数乘 4），代价是需要历史缓冲、
//! 需要知道"上一帧这个像素在哪"，以及在信息对不上时不累计。
//!
//! ```text
//!  场景 pass（HDR，投影按 jitter_offset 抖动）
//!        │
//!        ▼
//!   当前帧颜色 ──┐
//!                ├──▶ TAA resolve ──▶ 新的历史 ──▶ 曝光 / Bloom / 色调映射
//!   历史帧颜色 ──┘        ▲
//!   （按重投影 uv 采样）   │
//!                  3×3 邻域钳制 + 首帧 / 越界直通
//! ```
//!
//! # 几个必须写清楚的决定
//!
//! - **抖动加在相机投影上，不加在深度 / 剔除 / 阴影拟合上**：`ScenePipeline::render`
//!   收到的是**未抖动**的 `view_proj` 和一个 NDC 偏移量，抖动只在写相机 uniform
//!   时叠加。否则级联阴影的纹素吸附（整数纹素宽度 + 中心吸附）会被半个像素
//!   来回打破，表现为影子边缘每帧抖一下 —— 那是比锯齿更难看的瑕疵。
//! - **重投影用"未抖动的上一帧 view_proj + 上一帧抖动偏移"**：世界点先按未抖动
//!   矩阵投到上一帧的 uv，再加上那一帧的抖动（渲染时画面被移过），才是历史纹理
//!   里真正存着这个点的地方。两者少一个都会差半个像素 —— 差分是"整体轻微模糊"，
//!   不会报错，是最容易漏的一处。
//! - **历史必须钳制在当前帧邻域的 [min, max] 里**：v1 没有逐物体的运动向量，
//!   重投影只跟相机走；钳制是"运动物体不拖影"的主要防线（也是唯一的防线）。
//! - **首帧 / 重投影越界 / 相机在背后**：一律**直通当前帧**（blend = 0），不猜。
//!   猜错的代价是画面里出现上一帧的鬼影，而不猜的代价只是这一帧没抗锯齿。
//! - **纹理是乒乓的**：同一张纹理不能既当输入又当输出。每帧读上一帧写的那张、
//!   写另一张，所以历史纹理需要两张、后处理绑定组需要 2 个缓存槽。
//! - **深度用 `textureLoad` 而不是采样**：深度纹理不能用过滤采样器，而且 1:1
//!   的后处理本来就要精确到像素。MSAA 开着时读 0 号采样点（重投影只需要一个
//!   代表性的深度，四个采样点全读反而是四倍开销）。
//! - **默认关**：与 Bloom / 天空 / FXAA 同一条纪律。关着时整段跳过 —— 不建
//!   历史纹理、不记 pass、投影不抖动，画面与 TAA 上线前逐位一致。

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use xgame_math::glam::{Mat4, Vec3};

use xgame_core::TaaConfig;

use crate::postfx::HDR_FORMAT;

/// 一个抖动周期内的采样数（Halton(2,3) 的前 N 项）。
///
/// 8 是"肉眼几乎看不出收敛过程"与"收敛够快（相机停下 8 帧内边缘就干净）"的
/// 折中。改成 16 会让静态画面的收敛更彻底，但相机停下后的过渡更长。
pub const JITTER_SAMPLES: u32 = 8;

/// 邻域钳制的放大系数（1.0 = 严格钳在 3×3 的 [min, max] 内）。
///
/// 放宽会让历史更容易通过、画面更稳，但运动物体的残影也更长；收紧（< 1）
/// 会把邻域本身的有效范围压扁，等于"更快地忘掉历史"，静止画面的抗锯齿会变差。
const NEIGHBORHOOD_SCALE: f32 = 1.0;

// ---------------------------------------------------------------------------
// 纯函数：抖动序列 / 抖动矩阵 / 重投影（都可单测）
// ---------------------------------------------------------------------------

/// Halton 序列第 `index` 项（**1 起**），`base` 为质数基数。
///
/// 反射式基数展开：把 index 按 base 写成小数位再镜像到小数点右边。
/// 这里不用随机数：抖动序列必须**逐帧可复现**（无头截图两次运行要逐字节一致，
/// 回归测试也要能钉住"第 k 帧抖多少"）。
pub fn halton(mut index: u32, base: u32) -> f32 {
    debug_assert!(base >= 2);
    let mut f = 1.0f32;
    let mut result = 0.0f32;
    while index > 0 {
        f /= base as f32;
        result += f * (index % base) as f32;
        index /= base;
    }
    result
}

/// 第 `frame` 帧的抖动偏移，单位 = **像素**（屏幕空间，y 朝下），范围 (-0.5, 0.5]。
///
/// 用二维 Halton（基数 2 / 3）而不是"螺旋采样"：两者的分布质量相当，但 Halton
/// 是纯算术、无状态，任意帧可以直接算出偏移 —— 中途重置历史不需要重置序列。
pub fn jitter_offset(frame: u32) -> [f32; 2] {
    let i = frame % JITTER_SAMPLES + 1;
    [halton(i, 2) - 0.5, halton(i, 3) - 0.5]
}

/// 像素抖动 → 裁剪空间（NDC）平移量。
///
/// NDC 的 x / y 都覆盖 2 个单位、对应 `size` 个像素 ⇒ 一个像素 = 2 / size。
pub fn jitter_to_ndc(jitter_px: [f32; 2], size: (u32, u32)) -> [f32; 2] {
    if size.0 == 0 || size.1 == 0 {
        return [0.0, 0.0];
    }
    [
        2.0 * jitter_px[0] / size.0 as f32,
        2.0 * jitter_px[1] / size.1 as f32,
    ]
}

/// 像素抖动 → 纹理空间（uv）平移量。
///
/// **y 要取反**：NDC 的 y 朝上、纹理 v 朝下。这一步写成 `-jy / h` 而不是
/// `jy / h`，是重投影能不能对上的分水岭（写反 = 历史永远差两个像素，画面
/// 糊成一团但不会报错）。
pub fn jitter_to_uv(jitter_px: [f32; 2], size: (u32, u32)) -> [f32; 2] {
    if size.0 == 0 || size.1 == 0 {
        return [0.0, 0.0];
    }
    [jitter_px[0] / size.0 as f32, -jitter_px[1] / size.1 as f32]
}

/// 把 NDC 平移量塞进投影矩阵。
///
/// 目标：`clip.x += dx · clip.w`、`clip.y += dy · clip.w` —— 除以 w 之后
/// 正好是 NDC 上的固定平移（透视除法不会把它按深度缩放）。
/// 矩阵的行 0 / 行 1 各自加上 `d · 行 3` 即可；对标准透视矩阵，行 3 只有
/// z 分量非零 ⇒ 实际只动 `[2][0]` / `[2][1]` 两个元素（经典写法），
/// 但这里按"行 += d·行3"的一般形式写，换成别的投影也不会静默失效。
pub fn jitter_projection(mut proj: Mat4, jitter_ndc: [f32; 2]) -> Mat4 {
    if jitter_ndc == [0.0, 0.0] {
        return proj;
    }
    for c in 0..4 {
        let w = proj.col(c).w;
        proj.col_mut(c).x += jitter_ndc[0] * w;
        proj.col_mut(c).y += jitter_ndc[1] * w;
    }
    proj
}

// ---------------------------------------------------------------------------
// uniform
// ---------------------------------------------------------------------------

/// TAA resolve 的 uniform（必须与 WGSL 逐字节对齐）。
///
/// 两个 `mat4x4` 是主体（各 64 B）；后面的标量按 WGSL 的对齐规则排，
/// 末尾 `resolution` 恰好把总长补到 16 的倍数（176 B）。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct TaaParams {
    /// **抖动后**当前帧的逆 view_proj（把像素 + 深度反投影回世界）。
    inv_view_proj: [[f32; 4]; 4],
    /// **未抖动**的上一帧 view_proj（世界 → 上一帧 uv，抖动另行叠加）。
    prev_view_proj: [[f32; 4]; 4],
    /// 相机位置（天空像素的方向重投影要用）。
    camera_pos: [f32; 4],
    /// 上一帧的抖动偏移（uv 单位，已含 y 取反）。
    prev_jitter_uv: [f32; 2],
    /// 源纹理的 texel 尺寸（1 / 分辨率）。
    texel: [f32; 2],
    /// 历史权重：0 = 完全不用历史（首帧 / 重投影失败时）。
    blend: f32,
    /// 邻域钳制的放大系数。
    clamp_scale: f32,
    /// 分辨率（像素），邻域采样要按它夹边界。
    resolution: [f32; 2],
}

// ---------------------------------------------------------------------------
// resolve 管线
// ---------------------------------------------------------------------------

/// 一条全屏 pipeline：当前颜色 + 历史颜色 + 深度 + uniform → 新的历史。
struct TaaPipeline {
    bgl: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
}

impl TaaPipeline {
    fn new(device: &wgpu::Device, sample_count: u32) -> Self {
        // 深度附件是否多采样，决定了 WGSL 里深度纹理的类型（`texture_depth_2d`
        // 还是 `texture_depth_multisampled_2d`）。两种类型的 `textureLoad` 调用
        // 都是 `(t, coords, 0)`，所以着色器只有这一行不同 —— 用占位符替换，
        // 免得维护两份几乎一样的 WGSL（迟早漂移）。
        let multisampled_depth = sample_count > 1;
        let depth_type = if multisampled_depth {
            "texture_depth_multisampled_2d"
        } else {
            "texture_depth_2d"
        };
        let shader_src = TAA_WGSL.replace("__DEPTH_TYPE__", depth_type);

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
            label: Some("taa_bgl"),
            entries: &[
                // 0: 当前帧颜色
                tex_entry(0),
                // 1: 深度（**不可过滤** —— 深度纹理只允许 NonFiltering 采样器，
                //    本 pass 也不需要采样器：全部走 textureLoad）
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: multisampled_depth,
                    },
                    count: None,
                },
                // 2: 历史颜色
                tex_entry(2),
                // 3: 采样器（颜色用线性：重投影 uv 一般不在像素中心）
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // 4: uniform
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
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
            label: Some("taa_layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("taa_shader"),
            source: wgpu::ShaderSource::Wgsl(shader_src.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("taa_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    // 不混合：resolve 的输出就是最终的新历史（混合在着色器里做）。
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            // resolve 是单采样全屏 pass，与场景 pass 的 MSAA 无关。
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self { bgl, pipeline }
    }
}

/// 两张 HDR 历史纹理（乒乓）+ 这一帧该写哪张。
struct TaaHistory {
    size: (u32, u32),
    _textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
    /// 本帧要写入的下标（= 1 − 上一帧写入的下标）。
    write: usize,
}

/// TAA 的 GPU 状态与帧间状态。
///
/// 三宿主（编辑器视口 / 独立游戏 / 无头截图）各持一份 —— 历史缓冲是**每张
/// 显示目标**的，共用一份会让两个视口互相污染。
pub struct TaaResolver {
    pipeline: TaaPipeline,
    uniform: wgpu::Buffer,
    sampler: wgpu::Sampler,
    history: Option<TaaHistory>,
    /// 已推进的帧数（只增不减，抖动序列按它取模）。
    frame: u32,
    /// 距离上次"历史作废"过了几帧：0 = 本帧必须直通（历史里没有可用内容）。
    frames_since_reset: u32,
    /// 本帧的抖动（`begin_frame` 算好，`resolve` 用它反投影）。
    jitter: [f32; 2],
    /// 上一帧的抖动（重投影要加它 —— 见 [`jitter_to_uv`]）。
    prev_jitter: [f32; 2],
    /// 上一帧**未抖动**的 view_proj。
    prev_view_proj: Option<Mat4>,
    /// 绑定组缓存：两张历史纹理各一个槽（乒乓），键是 `bind_gen`。
    /// 绑定组里钉着颜色 / 深度视图，目标重建时必须失效（`reset` 里自增代号）。
    bind_cache: [Option<(u64, wgpu::BindGroup)>; 2],
    /// 绑定代：目标重建 / 历史重建时自增，令缓存失效。
    bind_gen: u64,
}

impl TaaResolver {
    /// `sample_count` 是**场景 pass** 的采样数：深度附件的类型按它选
    /// （多采样深度要用 `texture_depth_multisampled_2d`，否则绑定组校验失败）。
    pub fn new(device: &wgpu::Device, sample_count: u32) -> Self {
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("taa_params"),
            contents: bytemuck::bytes_of(&TaaParams {
                inv_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                prev_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                camera_pos: [0.0; 4],
                prev_jitter_uv: [0.0; 2],
                texel: [1.0, 1.0],
                blend: 0.0,
                clamp_scale: NEIGHBORHOOD_SCALE,
                resolution: [1.0, 1.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("taa_sampler"),
            // 历史按重投影 uv 采样，必然落在像素之间 → 线性过滤。
            // 越界一律夹边（越界的情况在着色器里已经直通，这里只是兜底，
            // 免得出现 ClampToEdge 之外的重复边框色）。
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            pipeline: TaaPipeline::new(device, sample_count.max(1)),
            uniform,
            sampler,
            history: None,
            frame: 0,
            frames_since_reset: 0,
            jitter: [0.0, 0.0],
            prev_jitter: [0.0, 0.0],
            prev_view_proj: None,
            bind_cache: [None, None],
            bind_gen: 0,
        }
    }

    /// 历史作废：目标重建（视图换了）/ 开关切换 / 场景切换 / 相机瞬移时调用。
    ///
    /// 不丢已有纹理（尺寸多半没变，下一帧直接覆盖），只把"还能不能相信历史"
    /// 的状态清掉，并让绑定组缓存失效（它钉着旧的颜色 / 深度视图）。
    pub fn reset(&mut self) {
        self.frames_since_reset = 0;
        self.prev_view_proj = None;
        self.bind_gen = self.bind_gen.wrapping_add(1);
        self.bind_cache = [None, None];
    }

    /// 本帧应当使用的抖动偏移（像素）。**每渲染一帧调用一次**。
    ///
    /// 尺寸变化时顺手重建历史（逐像素都对不上了，留着只是浪费显存）。
    pub fn begin_frame(&mut self, size: (u32, u32)) -> [f32; 2] {
        if self.history.as_ref().map(|h| h.size) != Some(size.max((1, 1))) {
            self.drop_history();
        }
        let jitter = jitter_offset(self.frame);
        self.frame = self.frame.wrapping_add(1);
        self.jitter = jitter;
        jitter
    }

    /// 历史纹理的尺寸（调试 / 测试用；没建过就是 `None`）。
    pub fn history_size(&self) -> Option<(u32, u32)> {
        self.history.as_ref().map(|h| h.size)
    }

    /// `resolve` 返回的那张视图的**源代号**，给 `PostFx` 的绑定组缓存当键。
    ///
    /// 乒乓意味着源视图每帧都换：代号必须跟着换（否则后处理会拿着上一帧的
    /// 绑定组去读另一张纹理，画面恒定落后一帧）。最高位固定为 1，与宿主自己的
    /// `target_gen`（小整数）永不相撞 —— 两者混用同一个缓存表，撞了就是
    /// "绑定组指向别的纹理"这类静默错误。
    fn output_gen(&self, index: usize) -> u64 {
        (1u64 << 63) | (self.bind_gen << 1) | index as u64
    }

    /// 记录 resolve pass，返回**本帧的新历史**视图与它的源代号。
    ///
    /// 代号一并返回（而不是让调用方事后再问）：`resolve` 之后借用还挂在
    /// `self` 上，再去调 `&self` 的方法会直接编译不过 —— 签名上一次性给全，
    /// 调用点就不可能写错。
    ///
    /// `view_proj` 必须是**未抖动**的那个（与传给 `ScenePipeline::render` 的一致）。
    #[allow(clippy::too_many_arguments)]
    pub fn resolve(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        size: (u32, u32),
        view_proj: &Mat4,
        camera_pos: &Vec3,
        config: TaaConfig,
    ) -> (&wgpu::TextureView, u64) {
        let size = size.max((1, 1));
        self.ensure_history(device, size);

        let history = self.history.as_ref().expect("刚确保过历史纹理");
        // `write` = 本帧的写入目标（也就是本帧的**输出**）、`read` = 上一帧写的
        // 那张（历史）。两个都只是在"翻牌之前"才有意义 —— 本函数末尾会把
        // `history.write` 改指向下一帧要写的位置。
        let write = history.write;
        let read = 1 - write;

        // 统一一次：非法 view_proj（NaN / 退化）会让反投影出垃圾坐标，
        // 那样连"直通当前帧"都救不了画面（采样到纹理外的一团）。退化时
        // 直接当"没有历史"处理。
        let invertible = view_proj.determinant().is_finite() && view_proj.determinant() != 0.0;
        let jitter_ndc = jitter_to_ndc(self.jitter, size);
        let jittered_vp = jitter_projection(*view_proj, jitter_ndc);
        let inv_view_proj = jittered_vp.inverse();

        // 没有可用历史（刚重置 / 上一帧矩阵不可信 / view_proj 退化）→ 直通。
        let blend = match self.prev_view_proj {
            Some(_) if self.frames_since_reset > 0 && invertible => config
                .history_weight
                .clamp(0.0, TaaConfig::MAX_HISTORY_WEIGHT),
            _ => 0.0,
        };

        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&TaaParams {
                inv_view_proj: inv_view_proj.to_cols_array_2d(),
                prev_view_proj: self.prev_view_proj.unwrap_or(*view_proj).to_cols_array_2d(),
                camera_pos: [camera_pos.x, camera_pos.y, camera_pos.z, 1.0],
                prev_jitter_uv: jitter_to_uv(self.prev_jitter, size),
                texel: [1.0 / size.0 as f32, 1.0 / size.1 as f32],
                blend,
                clamp_scale: NEIGHBORHOOD_SCALE,
                resolution: [size.0 as f32, size.1 as f32],
            }),
        );

        // 绑定组按（代号, 写哪张）缓存：乒乓 ⇒ 每帧换一个槽，两个槽就够。
        let needs_rebuild = self.bind_cache[write]
            .as_ref()
            .map(|(gen, _)| *gen != self.bind_gen)
            .unwrap_or(true);
        if needs_rebuild {
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("taa_bg"),
                layout: &self.pipeline.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(color_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(depth_view),
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
            self.bind_cache[write] = Some((self.bind_gen, bg));
        }
        let bind_group = &self.bind_cache[write].as_ref().expect("刚写入过缓存").1;

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("taa_resolve"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &history.views[write],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // 全屏三角形铺满目标；清黑只为"没被覆盖的边界像素"有确定值。
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline.pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // 帧间状态推进：这一帧写的那张，下一帧当历史读。
        if let Some(history) = self.history.as_mut() {
            history.write = read;
        }
        self.prev_view_proj = Some(*view_proj);
        self.prev_jitter = self.jitter;
        self.frames_since_reset = self.frames_since_reset.saturating_add(1);

        // 返回的是**本帧刚写的那张**（`write`），不是刚读的那张 —— 读错一张
        // 的后果是"后处理恒定落后一帧"，而首帧那张还没被写过，整屏全黑。
        let gen = self.output_gen(write);
        let history = self.history.as_ref().expect("刚用过历史纹理");
        (&history.views[write], gen)
    }

    fn drop_history(&mut self) {
        self.history = None;
        self.frames_since_reset = 0;
        self.prev_view_proj = None;
        self.bind_gen = self.bind_gen.wrapping_add(1);
        self.bind_cache = [None, None];
    }

    fn ensure_history(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.history.as_ref().map(|h| h.size) == Some(size) {
            return;
        }
        let make = |label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FORMAT,
                // 两张都是"这帧写、下帧读"—— 两种用途都要。
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let textures = [make("taa_history_a"), make("taa_history_b")];
        let views = [
            textures[0].create_view(&wgpu::TextureViewDescriptor::default()),
            textures[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];
        self.history = Some(TaaHistory {
            size,
            _textures: textures,
            views,
            write: 0,
        });
        // 新纹理里没有可用内容 ⇒ 下一帧必须直通。
        self.frames_since_reset = 0;
        self.prev_view_proj = None;
        self.bind_gen = self.bind_gen.wrapping_add(1);
        self.bind_cache = [None, None];
    }
}

// ---------------------------------------------------------------------------
// WGSL
// ---------------------------------------------------------------------------

const TAA_WGSL: &str = r#"
struct TaaParams {
    inv_view_proj: mat4x4<f32>,
    prev_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    prev_jitter_uv: vec2<f32>,
    texel: vec2<f32>,
    blend: f32,
    clamp_scale: f32,
    resolution: vec2<f32>,
}

@group(0) @binding(0) var current_tex: texture_2d<f32>;
@group(0) @binding(1) var depth_tex: __DEPTH_TYPE__;
@group(0) @binding(2) var history_tex: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
@group(0) @binding(4) var<uniform> params: TaaParams;

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
    // 纹理坐标 v 朝下、NDC 的 y 朝上 —— 翻一次（与 postfx / fxaa 同一约定）。
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

@fragment
fn fs_main(input: VsOut) -> @location(0) vec4<f32> {
    let uv = input.uv;
    let cur = textureSampleLevel(current_tex, samp, uv, 0.0);

    // 没有可用历史（首帧 / 刚重置 / 重投影失败）→ 直通，别猜。
    if (params.blend <= 0.0) {
        return cur;
    }

    // 像素整数坐标：`clip.xy` 已经是像素中心（x + 0.5），截断即得像素下标。
    let pixel = vec2<i32>(input.clip.xy);
    let max_pixel = vec2<i32>(params.resolution) - vec2<i32>(1, 1);

    // --- 反投影：像素 + 深度 → 世界 ---
    // 深度是**抖动后**的投影渲出来的，所以要用抖动后的逆矩阵反推。
    let d = textureLoad(depth_tex, pixel, 0);
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let h = params.inv_view_proj * vec4<f32>(ndc, d, 1.0);
    var world = h.xyz / h.w;

    // 远平面（天空 / 没有几何的地方）：深度几乎是 1，反投影出来是一条射线上的
    // 任意点。按**视线方向**把它推到远处再重投影 —— 相机旋转能对上，平移的
    // 视差忽略（天空本来就在无穷远）。
    if (d >= 0.99999) {
        let dir = normalize(world - params.camera_pos.xyz);
        world = params.camera_pos.xyz + dir * 1.0e4;
    }

    // --- 重投影到上一帧 ---
    let pc = params.prev_view_proj * vec4<f32>(world, 1.0);
    if (pc.w <= 1.0e-6) {
        // 落在上一帧相机背后 —— 历史里没有它的位置。
        return cur;
    }
    let prev_ndc = pc.xy / pc.w;
    // 上一帧的抖动必须补上：那一帧的画面是被平移过的。
    let prev_uv = vec2<f32>(prev_ndc.x * 0.5 + 0.5, 0.5 - prev_ndc.y * 0.5) + params.prev_jitter_uv;
    if (prev_uv.x < 0.0 || prev_uv.x > 1.0 || prev_uv.y < 0.0 || prev_uv.y > 1.0) {
        return cur;
    }

    // --- 3×3 邻域钳制 ---
    // 历史颜色必须落在"当前帧这个像素附近真实存在的颜色范围"内：超出范围
    // 说明历史讲的是别的东西（遮挡变了 / 物体移开了 / 相机甩得太快），
    // 钳到边界就等于"部分接受"，比整块丢弃平滑得多。
    var lo = cur.rgb;
    var hi = cur.rgb;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let q = clamp(pixel + vec2<i32>(x, y), vec2<i32>(0, 0), max_pixel);
            let c = textureLoad(current_tex, q, 0).rgb;
            lo = min(lo, c);
            hi = max(hi, c);
        }
    }
    let spread = (hi - lo) * params.clamp_scale;
    var hist = textureSampleLevel(history_tex, samp, prev_uv, 0.0);
    hist = vec4<f32>(clamp(hist.rgb, lo - spread, hi + spread), hist.a);

    // blend = 历史权重：0.9 表示"这一帧只进 10%"。
    return mix(cur, hist, params.blend);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use xgame_math::glam::Vec4;

    /// Halton 前几项是手算得到的定值 —— 序列一旦变了，历史帧的位置就全变，
    /// 无头截图与回归基线会一起漂。
    #[test]
    fn halton_first_terms_are_the_known_sequence() {
        assert!((halton(1, 2) - 0.5).abs() < 1e-6);
        assert!((halton(2, 2) - 0.25).abs() < 1e-6);
        assert!((halton(3, 2) - 0.75).abs() < 1e-6);
        assert!((halton(1, 3) - 1.0 / 3.0).abs() < 1e-6);
        assert!((halton(2, 3) - 2.0 / 3.0).abs() < 1e-6);
        assert!((halton(3, 3) - 1.0 / 9.0).abs() < 1e-6);
    }

    /// 抖动必须在 (-0.5, 0.5] 像素内、周期为 `JITTER_SAMPLES`，且一周期内
    /// **不重复**（重复的子像素位置等于白采样一次）。
    #[test]
    fn jitter_covers_a_unique_half_pixel_grid() {
        let mut seen = Vec::new();
        for f in 0..JITTER_SAMPLES {
            let j = jitter_offset(f);
            for v in j {
                assert!(v > -0.5 && v <= 0.5, "抖动越界: {v}");
            }
            assert!(
                !seen.contains(&j),
                "一个周期内出现重复抖动: {j:?} (frame {f})"
            );
            seen.push(j);
        }
        // 周期性：第 N + JITTER_SAMPLES 帧与第 N 帧完全一样。
        assert_eq!(
            jitter_offset(0),
            jitter_offset(JITTER_SAMPLES),
            "抖动序列必须是定长周期（可复现性的地基）"
        );
    }

    /// 抖动投影：NDC 上的位移必须**精确**等于 2·jitter/size，且与深度无关
    /// （透视除法把 clip 上的平移变成了 NDC 上的常量位移）。
    #[test]
    fn jitter_projection_shifts_ndc_by_exactly_the_requested_amount() {
        let proj = Mat4::perspective_rh(60f32.to_radians(), 16.0 / 9.0, 0.1, 100.0);
        let size = (1600u32, 900u32);
        let jitter_px = [0.5, -0.25];
        let ndc = jitter_to_ndc(jitter_px, size);
        let jittered = jitter_projection(proj, ndc);

        for z in [0.2f32, 1.0, 5.0, 40.0] {
            for (x, y) in [(0.0f32, 0.0f32), (0.7, -0.3), (-2.0, 1.5)] {
                let p = Vec4::new(x, y, z, 1.0);
                let a = proj * p;
                let b = jittered * p;
                let (na, nb) = (
                    xgame_math::glam::Vec2::new(a.x / a.w, a.y / a.w),
                    xgame_math::glam::Vec2::new(b.x / b.w, b.y / b.w),
                );
                let d = nb - na;
                assert!(
                    (d.x - ndc[0]).abs() < 1e-5 && (d.y - ndc[1]).abs() < 1e-5,
                    "z={z}: NDC 位移 {d:?} 与请求的 {ndc:?} 不符"
                );
            }
        }
    }

    /// 零抖动必须是**恒等变换**（逐位相同）：关着 TAA 时不能有任何画质漂移。
    #[test]
    fn zero_jitter_is_bit_identical() {
        let proj = Mat4::perspective_rh(60f32.to_radians(), 1.5, 0.1, 100.0);
        let same = jitter_projection(proj, [0.0, 0.0]);
        assert_eq!(proj.to_cols_array(), same.to_cols_array());
    }

    /// uv 与 NDC 的 y 方向相反：像素抖动 → uv 平移必须取反 y。
    ///
    /// 这条钉的是"重投影差两个像素、画面糊成一团但不报错"那类最难查的错
    /// —— 当年 FXAA 与 postfx 都在这条上吃过亏（v 朝下）。
    #[test]
    fn jitter_uv_flips_y_compared_to_ndc() {
        let size = (800u32, 600u32);
        let jitter = [0.25, 0.4];
        let ndc = jitter_to_ndc(jitter, size);
        let uv = jitter_to_uv(jitter, size);
        assert!((ndc[0] * 0.5 - uv[0]).abs() < 1e-7, "x 同向");
        assert!((ndc[1] * 0.5 + uv[1]).abs() < 1e-7, "y 必须反向");
    }

    /// 尺寸为 0 时不许除零（编辑器视口折叠成 0 宽是常态）。
    #[test]
    fn degenerate_size_yields_zero_jitter() {
        assert_eq!(jitter_to_ndc([0.3, 0.3], (0, 100)), [0.0, 0.0]);
        assert_eq!(jitter_to_uv([0.3, 0.3], (100, 0)), [0.0, 0.0]);
    }

    /// uniform 的 Rust 侧布局必须与 WGSL 一致（176 B，两个 mat4 之后按 8/4 对齐）。
    ///
    /// 这类错误在跑起来之前完全看不出来：着色器读到的是错位后的值，
    /// 表现为"历史采样位置全错"而不是崩溃（CSM 的 304 B 就是这么踩过的）。
    #[test]
    fn taa_params_matches_wgsl_layout() {
        assert_eq!(std::mem::size_of::<TaaParams>(), 176);
        assert_eq!(std::mem::align_of::<TaaParams>(), 4);
        let p = TaaParams {
            inv_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            prev_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            camera_pos: [1.0, 2.0, 3.0, 1.0],
            prev_jitter_uv: [0.5, -0.5],
            texel: [0.25, 0.5],
            blend: 0.75,
            clamp_scale: 1.0,
            resolution: [640.0, 480.0],
        };
        let bytes = bytemuck::bytes_of(&p);
        // camera_pos 从第 128 字节开始（两个 mat4 之后）。
        assert_eq!(&bytes[128..132], 1.0f32.to_le_bytes());
        // prev_jitter_uv 在第 144 字节。
        assert_eq!(&bytes[144..148], 0.5f32.to_le_bytes());
        // blend 在第 160 字节。
        assert_eq!(&bytes[160..164], 0.75f32.to_le_bytes());
        // resolution 在第 168 字节。
        assert_eq!(&bytes[168..172], 640.0f32.to_le_bytes());
    }
}
