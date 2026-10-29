//! SSAO（屏幕空间环境光遮蔽）：半分辨率深度遮蔽 + 模糊 + 乘回场景色。
//!
//! # 在管线里的位置
//!
//! ```text
//! 场景 pass（HDR + 深度）→ [TAA resolve] → 【SSAO 乘算】→ PostFx → LDR → UI
//! ```
//!
//! 与 [`crate::taa::TaaResolver`] 同款接口形态（`apply` 返回 `(视图, 代号)`，
//! 代号给 `PostFx` 的绑定组缓存当键）：三宿主插入一行就能接上，`PostFx`
//! 与既有基准**零改动**。乘算写进自己的全分辨率 scratch（HDR 格式），
//! 不就地改场景色 —— 读写同一张纹理是 wgpu 明令禁止的。
//!
//! # 半分辨率 + 深度重建
//!
//! AO 图在**半分辨率**上算（每像素只采一个全分辨率深度点），法线由深度
//! 梯度重建（每轴取深度差更小的一侧，跨轮廓不把远处表面拉进来）。没有
//! G-buffer 之前的权衡：平滑曲面的法线有梯度噪声，靠模糊压掉。
//!
//! # 确定性噪声
//!
//! kernel 旋转角来自**像素坐标的整数哈希**（[`hash_angle`]），没有随机
//! 纹理、没有时间项、没有随机种子进 GPU —— 同机同输入两次渲染逐字节一致，
//! `--screenshot` 的可复现契约在 SSAO 开着时也成立（有 GPU 测试钉住）。
//! 半球 kernel 本身由固定种子 LCG 在构建时生成（[`hemisphere_kernel`]）。
//!
//! # 关闭 = 零成本
//!
//! `SsaoConfig::is_active()` 为假时宿主**整段跳过**：不建纹理、不记 pass、
//! 不写 uniform。与 Bloom / 天空 / FXAA / TAA 同一条纪律。

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use xgame_core::SsaoConfig;
use xgame_math::glam::{Mat4, Vec3};

use crate::postfx::HDR_FORMAT;

/// 半球 kernel 的采样数。16 是"质量够、每像素开销看得住"的常用档。
const KERNEL_SAMPLES: usize = 16;

/// SSAO 全部 pass 共享的 uniform（必须与 WGSL 逐字节对齐，448 B）。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct SsaoParams {
    /// 深度 → 世界的逆矩阵（像素 + 深度反投影出世界坐标）。
    inv_view_proj: [[f32; 4]; 4],
    /// 世界 → 裁剪（把 kernel 采样点投回屏幕）。
    view_proj: [[f32; 4]; 4],
    /// 相机位置（距离比较式遮蔽判定要用）。
    camera_pos: [f32; 4],
    /// 半球 kernel（切空间 xyz，w 未用）。
    kernel: [[f32; 4]; KERNEL_SAMPLES],
    /// x=半径 y=偏置 z=强度 w=未用。
    params: [f32; 4],
    /// 全分辨率：xy=像素数 zw=1/像素数。
    res: [f32; 4],
    /// 半分辨率：xy=像素数 zw=1/像素数（模糊的边界夹取用）。
    half_res: [f32; 4],
}

/// 固定种子 LCG 生成半球 kernel（确定性：同一构建在任何机器上同一组向量）。
fn hemisphere_kernel() -> [[f32; 4]; KERNEL_SAMPLES] {
    // 种子与乘加常量都是随手定的质数 —— 重点是"写死"，不是"好"。
    let mut state: u32 = 0x2545_F491;
    let mut next = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        state
    };
    let mut out = [[0.0f32; 4]; KERNEL_SAMPLES];
    for (i, slot) in out.iter_mut().enumerate() {
        let x = (next() as f32 / u32::MAX as f32) * 2.0 - 1.0;
        let y = (next() as f32 / u32::MAX as f32) * 2.0 - 1.0;
        // z ∈ [0,1)：朝 +z 的半球（切空间法线方向）。
        let z = next() as f32 / u32::MAX as f32;
        let mut v = Vec3::new(x, y, z).normalize_or_zero();
        if v == Vec3::ZERO {
            v = Vec3::Z;
        }
        // 向原点聚拢（二次分布）：近处样本密，遮蔽边缘的渐变更稳。
        let t = (i as f32 + 1.0) / KERNEL_SAMPLES as f32;
        let scale = 0.1 + 0.9 * t * t;
        *slot = [v.x * scale, v.y * scale, v.z * scale, 0.0];
    }
    out
}

// ---------------------------------------------------------------------------
// shader（模块内字符串常量，与 postfx / taa 同一惯例）
// ---------------------------------------------------------------------------

/// 全屏三角形顶点着色器（三个 pass 共用同一段 vs 源码字符串拼接）。
const SSAO_VS: &str = r#"
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
    // 纹理坐标 v 朝下、NDC 的 y 朝上 —— 翻一次（与 taa / postfx 同一约定）。
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}
"#;

/// AO 生成（半分辨率输出）。`__DEPTH_TYPE__` 按采样数替换（同 taa 的做法）。
const SSAO_AO_WGSL: &str = r#"
struct SsaoParams {
    inv_view_proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    kernel: array<vec4<f32>, 16>,
    params: vec4<f32>,
    res: vec4<f32>,
    half_res: vec4<f32>,
}

@group(0) @binding(0) var depth_tex: __DEPTH_TYPE__;
@group(0) @binding(1) var<uniform> u: SsaoParams;

fn world_from_depth(pixel: vec2<i32>) -> vec3<f32> {
    let uv = (vec2<f32>(pixel) + vec2<f32>(0.5, 0.5)) * u.res.zw;
    let d = textureLoad(depth_tex, pixel, 0);
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let h = u.inv_view_proj * vec4<f32>(ndc, d, 1.0);
    return h.xyz / h.w;
}

/// 像素整数哈希 → [0,1) 的旋转角比例。纯整数运算，跨运行逐位一致。
fn hash_angle(p: vec2<i32>) -> f32 {
    var h = u32(p.x) * 1973u + u32(p.y) * 9277u + 1u;
    h = (h ^ (h >> 13u)) * 1274126177u;
    return f32(h & 0xFFFFu) / 65536.0;
}

@fragment
fn fs_main(input: VsOut) -> @location(0) vec4<f32> {
    // 强度 0 = 无遮蔽效果：整个计算段跳过（宿主在矩阵退化时也走这条
    // 路 —— 反投影出垃圾坐标时遮蔽只会是噪声，直通才是对的）。
    if (u.params.z <= 0.0) {
        return vec4<f32>(1.0, 1.0, 1.0, 1.0);
    }
    let half_px = vec2<i32>(input.clip.xy);
    let px = half_px * 2;
    let max_px = vec2<i32>(u.res.xy) - vec2<i32>(1, 1);
    let cpx = clamp(px, vec2<i32>(0), max_px);
    let d = textureLoad(depth_tex, cpx, 0);
    // 天空 / 没有几何：不遮蔽。
    if (d >= 0.99999) {
        return vec4<f32>(1.0, 1.0, 1.0, 1.0);
    }
    let p = world_from_depth(cpx);

    // 深度梯度重建法线：每轴取深度差更小的一侧（跨轮廓时不把远处表面拉进来）。
    let r_px = clamp(cpx + vec2<i32>(1, 0), vec2<i32>(0), max_px);
    let l_px = clamp(cpx - vec2<i32>(1, 0), vec2<i32>(0), max_px);
    let d_px = clamp(cpx + vec2<i32>(0, 1), vec2<i32>(0), max_px);
    let u_px = clamp(cpx - vec2<i32>(0, 1), vec2<i32>(0), max_px);
    let dr = textureLoad(depth_tex, r_px, 0);
    let dl = textureLoad(depth_tex, l_px, 0);
    let dd = textureLoad(depth_tex, d_px, 0);
    let du = textureLoad(depth_tex, u_px, 0);
    let dx = select(p - world_from_depth(l_px), world_from_depth(r_px) - p, abs(dr - d) < abs(d - dl));
    let dy = select(p - world_from_depth(u_px), world_from_depth(d_px) - p, abs(dd - d) < abs(d - du));
    var n = normalize(cross(dx, dy));
    let to_cam = u.camera_pos.xyz - p;
    if (dot(n, to_cam) < 0.0) {
        n = -n;
    }

    // 切线基 + 像素哈希旋转（确定性噪声）。
    let angle = hash_angle(cpx) * 6.2831853;
    let ca = cos(angle);
    let sa = sin(angle);
    let up = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.z) < 0.999);
    let tangent = normalize(cross(up, n));
    let bitangent = cross(n, tangent);
    let t = tangent * ca + bitangent * sa;
    let b = bitangent * ca - tangent * sa;

    let radius = u.params.x;
    let bias = u.params.y;
    let strength = u.params.z;
    let center_dist = length(to_cam);
    var occluded = 0.0;
    for (var i = 0; i < 16; i = i + 1) {
        let k = u.kernel[i].xyz;
        let s = p + (t * k.x + b * k.y + n * k.z) * radius;
        let clip = u.view_proj * vec4<f32>(s, 1.0);
        if (clip.w <= 1.0e-5) {
            continue;
        }
        let ndc = clip.xy / clip.w;
        let suv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        if (suv.x < 0.0 || suv.x > 1.0 || suv.y < 0.0 || suv.y > 1.0) {
            continue;
        }
        let spx = clamp(vec2<i32>(suv * u.res.xy), vec2<i32>(0), max_px);
        let sd = textureLoad(depth_tex, spx, 0);
        if (sd >= 0.99999) {
            continue;
        }
        let scene = world_from_depth(spx);
        let scene_dist = length(scene - u.camera_pos.xyz);
        let sample_dist = length(s - u.camera_pos.xyz);
        // 场景面比采样点更近（超出偏置）→ 采样点被挡。
        // 距离衰减：遮挡物与中心点深度差太远时视为"别的物体"，不算遮蔽
        // （Crytek 式 range check）。
        if (sample_dist - scene_dist > bias) {
            let range = smoothstep(0.0, 1.0, radius / max(abs(scene_dist - center_dist), 1.0e-4));
            occluded = occluded + range;
        }
    }
    let ao = 1.0 - clamp(occluded / 16.0 * strength, 0.0, 1.0);
    return vec4<f32>(ao, ao, ao, 1.0);
}
"#;

/// 可分离 5 抽头 [1,4,6,4,1]/16 模糊（半分辨率，横竖各一趟）。
const SSAO_BLUR_WGSL: &str = r#"
struct SsaoParams {
    inv_view_proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    kernel: array<vec4<f32>, 16>,
    params: vec4<f32>,
    res: vec4<f32>,
    half_res: vec4<f32>,
}

@group(0) @binding(0) var ao_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> u: SsaoParams;

fn blur(input: VsOut, dir: vec2<i32>) -> vec4<f32> {
    let px = vec2<i32>(input.clip.xy);
    let max_px = vec2<i32>(u.half_res.xy) - vec2<i32>(1, 1);
    var weights = array<f32, 5>(1.0, 4.0, 6.0, 4.0, 1.0);
    var sum = 0.0;
    for (var i = -2; i <= 2; i = i + 1) {
        let q = clamp(px + dir * i, vec2<i32>(0), max_px);
        sum = sum + textureLoad(ao_tex, q, 0).r * weights[i + 2];
    }
    return vec4<f32>(sum / 16.0, 0.0, 0.0, 1.0);
}

@fragment
fn fs_h(input: VsOut) -> @location(0) vec4<f32> {
    return blur(input, vec2<i32>(1, 0));
}

@fragment
fn fs_v(input: VsOut) -> @location(0) vec4<f32> {
    return blur(input, vec2<i32>(0, 1));
}
"#;

/// 乘算：场景色 × AO（AO 按线性过滤上采样到全分辨率）→ scratch。
const SSAO_APPLY_WGSL: &str = r#"
struct SsaoParams {
    inv_view_proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    kernel: array<vec4<f32>, 16>,
    params: vec4<f32>,
    res: vec4<f32>,
    half_res: vec4<f32>,
}

@group(0) @binding(0) var color_tex: texture_2d<f32>;
@group(0) @binding(1) var ao_tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var<uniform> u: SsaoParams;

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

@fragment
fn fs_main(input: VsOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(input.clip.xy);
    let color = textureLoad(color_tex, px, 0);
    // 半分辨率 AO 线性上采样到全分辨率（顺带把 2×2 块的缝抹掉）。
    let ao = textureSampleLevel(ao_tex, samp, input.uv, 0.0).r;
    return vec4<f32>(color.rgb * ao, color.a);
}
"#;

// ---------------------------------------------------------------------------
// 管线
// ---------------------------------------------------------------------------

/// 一条全屏管线（AO 生成 / 模糊横 / 模糊纵 / 乘算 各一条）。
struct Pass {
    bgl: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
}

/// 构建一条全屏管线：`shader_src` 已含 vs + fs，入口名 `vs_main` / `fs_entry`。
fn make_pass(
    device: &wgpu::Device,
    label: &str,
    shader_src: &str,
    fs_entry: &str,
    entries: &[wgpu::BindGroupLayoutEntry],
    output_format: wgpu::TextureFormat,
) -> Pass {
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(&format!("{label}_bgl")),
        entries,
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(&format!("{label}_layout")),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&format!("{label}_shader")),
        source: wgpu::ShaderSource::Wgsl(shader_src.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(&format!("{label}_pipeline")),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some(fs_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format: output_format,
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
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });
    Pass { bgl, pipeline }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn float_tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// 半分辨率 AO 纹理对（raw / 模糊中间张）+ 全分辨率乘算 scratch。
struct SsaoTargets {
    /// 请求的全分辨率尺寸（scratch 的尺寸）。
    size: (u32, u32),
    half: (u32, u32),
    ao_views: [wgpu::TextureView; 2],
    _ao_textures: [wgpu::Texture; 2],
    scratch_view: wgpu::TextureView,
    _scratch: wgpu::Texture,
    /// 模糊的两个输入方向绑定组：0 = 输入 ao_a，1 = 输入 ao_b。
    blur_binds: [wgpu::BindGroup; 2],
}

/// SSAO 的 GPU 状态。三宿主各持一份（与 `TaaResolver` 同款理由：AO 依赖
/// 各自的深度 / 目标，共用会互相污染）。
pub struct SsaoPass {
    ao_pass: Pass,
    blur_h_pass: Pass,
    blur_v_pass: Pass,
    apply_pass: Pass,
    uniform: wgpu::Buffer,
    sampler: wgpu::Sampler,
    targets: Option<SsaoTargets>,
    /// AO 生成的绑定组（钉着深度视图），键 = `bind_gen`。
    ao_bind: Option<(u64, wgpu::BindGroup)>,
    /// 乘算绑定组缓存：键 = (源代号, 目标代)。源视图每帧换（TAA 乒乓）
    /// —— 与 PostFx 同一"多槽 + 代号"纪律。
    apply_cache: Vec<((u64, u64), wgpu::BindGroup)>,
    /// 绑定代：目标重建时自增，令钉着旧视图的缓存失效。
    bind_gen: u64,
    /// 固定种子半球 kernel（构建时生成一次，逐帧写进 uniform）。
    kernel: [[f32; 4]; KERNEL_SAMPLES],
}

impl SsaoPass {
    /// `sample_count` 是**场景 pass** 的采样数：深度纹理类型按它选
    /// （多采样深度要用 `texture_depth_multisampled_2d`，见 [`crate::taa`]）。
    pub fn new(device: &wgpu::Device, sample_count: u32) -> Self {
        let multisampled_depth = sample_count.max(1) > 1;
        let depth_type = if multisampled_depth {
            "texture_depth_multisampled_2d"
        } else {
            "texture_depth_2d"
        };
        let ao_src = format!("{SSAO_VS}{SSAO_AO_WGSL}").replace("__DEPTH_TYPE__", depth_type);
        let blur_src = format!("{SSAO_VS}{SSAO_BLUR_WGSL}");

        let ao_entries = [
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: multisampled_depth,
                },
                count: None,
            },
            uniform_entry(1),
        ];
        let ao_pass = make_pass(
            device,
            "ssao_ao",
            &ao_src,
            "fs_main",
            &ao_entries,
            wgpu::TextureFormat::R8Unorm,
        );
        let blur_entries = [float_tex_entry(0), uniform_entry(1)];
        let blur_h_pass = make_pass(
            device,
            "ssao_blur_h",
            &blur_src,
            "fs_h",
            &blur_entries,
            wgpu::TextureFormat::R8Unorm,
        );
        let blur_v_pass = make_pass(
            device,
            "ssao_blur_v",
            &blur_src,
            "fs_v",
            &blur_entries,
            wgpu::TextureFormat::R8Unorm,
        );
        let apply_entries = [
            float_tex_entry(0),
            float_tex_entry(1),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            uniform_entry(3),
        ];
        let apply_pass = make_pass(
            device,
            "ssao_apply",
            SSAO_APPLY_WGSL,
            "fs_main",
            &apply_entries,
            HDR_FORMAT,
        );

        let kernel = hemisphere_kernel();
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ssao_params"),
            contents: bytemuck::bytes_of(&SsaoParams {
                inv_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                camera_pos: [0.0; 4],
                kernel,
                params: [0.5, 0.025, 1.0, 0.0],
                res: [1.0; 4],
                half_res: [1.0; 4],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ssao_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            ao_pass,
            blur_h_pass,
            blur_v_pass,
            apply_pass,
            uniform,
            sampler,
            targets: None,
            ao_bind: None,
            apply_cache: Vec::new(),
            bind_gen: 0,
            kernel,
        }
    }

    /// 目标重建（视图换了）时调用：钉着旧视图的绑定组缓存全部失效。
    pub fn reset(&mut self) {
        self.bind_gen = self.bind_gen.wrapping_add(1);
        self.ao_bind = None;
        self.apply_cache.clear();
    }

    /// `apply` 返回的 scratch 视图的**源代号**（给 PostFx 的绑定组缓存当键）。
    ///
    /// 最高位规则与 [`crate::taa::TaaResolver`] 相同：代号必须与宿主的
    /// `target_gen`（小整数）、TAA 的输出代（bit 63）永不相撞 —— 三者混用
    /// PostFx 的同一张缓存表，撞了就是"绑定组指向别的纹理"的静默错误。
    /// SSAO 占 bit 62（bit 63 恒 0），与 TAA（bit 63 恒 1）天然不撞。
    fn output_gen(&self) -> u64 {
        (1u64 << 62) | self.bind_gen
    }

    fn ensure_targets(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        let size = size.max((1, 1));
        if self.targets.as_ref().map(|t| t.size) == Some(size) {
            return;
        }
        let half = ((size.0 / 2).max(1), (size.1 / 2).max(1));
        let make_ao = |label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: half.0,
                    height: half.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let ao_textures = [make_ao("ssao_ao_a"), make_ao("ssao_ao_b")];
        let ao_views = [
            ao_textures[0].create_view(&wgpu::TextureViewDescriptor::default()),
            ao_textures[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];
        let scratch = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ssao_scratch"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let scratch_view = scratch.create_view(&wgpu::TextureViewDescriptor::default());

        let blur_binds = [
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ssao_blur_bg_a"),
                layout: &self.blur_h_pass.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&ao_views[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.uniform.as_entire_binding(),
                    },
                ],
            }),
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ssao_blur_bg_b"),
                layout: &self.blur_h_pass.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&ao_views[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.uniform.as_entire_binding(),
                    },
                ],
            }),
        ];

        self.targets = Some(SsaoTargets {
            size,
            half,
            ao_views,
            _ao_textures: ao_textures,
            scratch_view,
            _scratch: scratch,
            blur_binds,
        });
        // 目标换代：AO 绑定（钉旧 AO 视图）与乘算缓存（钉旧 scratch）都作废。
        self.reset();
    }

    /// 记录 SSAO 四趟 pass（AO 生成 → 模糊横 → 模糊纵 → 乘算），返回
    /// **乘算输出**（scratch）视图与它的源代号。`source_view` 是 TAA
    /// resolve 之后（或 HDR 直出）的场景色。
    ///
    /// 代号一并返回（而不是让调用方事后再问）：`apply` 之后借用还挂在
    /// `self` 上 —— 与 [`crate::taa::TaaResolver::resolve`] 同一签名纪律。
    /// **输出恒为 scratch**：调用方不必分支两种来源。
    ///
    /// `view_proj` 退化（NaN / 不可逆）时把强度按 0 写进 uniform（AO 着色器
    /// 直接早退成"无遮蔽"）—— 乘算变成把源色原样抄进 scratch，画面不动，
    /// 与 TAA 对坏矩阵的处理语义一致。
    ///
    /// `view_proj` 用**未抖动**的那个（与宿主传给 `ScenePipeline::render` /
    /// `TaaResolver::resolve` 的一致）；深度是抖动投影渲出来的，反投影用未
    /// 抖动矩阵的误差在半像素内，远小于采样半径，靠模糊就压掉了 —— 换来
    /// `SsaoPass` 不必感知 TAA 的抖动序列。
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        source_gen: u64,
        depth_view: &wgpu::TextureView,
        size: (u32, u32),
        view_proj: &Mat4,
        camera_pos: &Vec3,
        config: SsaoConfig,
    ) -> (&wgpu::TextureView, u64) {
        // 矩阵退化时强制"无遮蔽"：AO 早退、乘算变直通，画面逐位不变。
        let invertible = view_proj.determinant().is_finite() && view_proj.determinant() != 0.0;
        let strength = if invertible { config.strength } else { 0.0 };

        self.ensure_targets(device, size);
        let size = size.max((1, 1));
        let targets = self.targets.as_ref().expect("刚确保过目标");
        let half = targets.half;

        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&SsaoParams {
                inv_view_proj: view_proj.inverse().to_cols_array_2d(),
                view_proj: view_proj.to_cols_array_2d(),
                camera_pos: [camera_pos.x, camera_pos.y, camera_pos.z, 1.0],
                kernel: self.kernel,
                params: [config.radius, config.bias, strength, 0.0],
                res: [
                    size.0 as f32,
                    size.1 as f32,
                    1.0 / size.0 as f32,
                    1.0 / size.1 as f32,
                ],
                half_res: [
                    half.0 as f32,
                    half.1 as f32,
                    1.0 / half.0 as f32,
                    1.0 / half.1 as f32,
                ],
            }),
        );

        // --- AO 生成（半分辨率） ---
        let ao_needs_rebuild = self
            .ao_bind
            .as_ref()
            .map(|(gen, _)| *gen != self.bind_gen)
            .unwrap_or(true);
        if ao_needs_rebuild {
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ssao_ao_bg"),
                layout: &self.ao_pass.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(depth_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.uniform.as_entire_binding(),
                    },
                ],
            });
            self.ao_bind = Some((self.bind_gen, bg));
        }
        {
            let ao_bind = &self.ao_bind.as_ref().expect("刚写入过缓存").1;
            let targets = self.targets.as_ref().expect("刚确保过目标");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ssao_ao"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.ao_views[0],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            pass.set_pipeline(&self.ao_pass.pipeline);
            pass.set_bind_group(0, ao_bind, &[]);
            pass.draw(0..3, 0..1);
        }

        // --- 模糊横：ao_a → ao_b；模糊纵：ao_b → ao_a（最终 AO 留在 ao_a） ---
        {
            let targets = self.targets.as_ref().expect("刚确保过目标");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ssao_blur_h"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.ao_views[1],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            pass.set_pipeline(&self.blur_h_pass.pipeline);
            pass.set_bind_group(0, &targets.blur_binds[0], &[]);
            pass.draw(0..3, 0..1);
        }
        {
            let targets = self.targets.as_ref().expect("刚确保过目标");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ssao_blur_v"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.ao_views[0],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            pass.set_pipeline(&self.blur_v_pass.pipeline);
            pass.set_bind_group(0, &targets.blur_binds[1], &[]);
            pass.draw(0..3, 0..1);
        }

        // --- 乘算：源色 × ao_a → scratch ---
        let apply_key = (source_gen, self.bind_gen);
        let needs_rebuild = !self.apply_cache.iter().any(|(key, _)| *key == apply_key);
        if needs_rebuild {
            let targets = self.targets.as_ref().expect("刚确保过目标");
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ssao_apply_bg"),
                layout: &self.apply_pass.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(source_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&targets.ao_views[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.uniform.as_entire_binding(),
                    },
                ],
            });
            if self.apply_cache.len() >= 8 {
                // 与 PostFx 同一纪律：槽满整体重来，别做 LRU。
                self.apply_cache.clear();
            }
            self.apply_cache.push((apply_key, bg));
        }
        let apply_bind = &self
            .apply_cache
            .iter()
            .find(|(key, _)| *key == apply_key)
            .expect("刚写入过缓存")
            .1;
        {
            let targets = self.targets.as_ref().expect("刚确保过目标");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ssao_apply"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.scratch_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            pass.set_pipeline(&self.apply_pass.pipeline);
            pass.set_bind_group(0, apply_bind, &[]);
            pass.draw(0..3, 0..1);
        }

        let targets = self.targets.as_ref().expect("刚用过目标");
        (&targets.scratch_view, self.output_gen())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// kernel 是固定种子生成的：两次生成必须逐字段一致（确定性契约的一半）。
    #[test]
    fn kernel_is_deterministic() {
        assert_eq!(hemisphere_kernel(), hemisphere_kernel());
    }

    /// kernel 采样点都在单位半球内（z ≥ 0、长度 ≤ 1，scale ≤ 1）。
    #[test]
    fn kernel_stays_in_hemisphere() {
        for slot in hemisphere_kernel() {
            let v = Vec3::new(slot[0], slot[1], slot[2]);
            assert!(slot[2] >= 0.0, "半球 z 必须非负: {slot:?}");
            assert!(v.length() <= 1.0001, "kernel 点不得超出单位球: {slot:?}");
        }
    }

    /// 像素哈希是纯函数：同坐标同值（确定性噪声的地基）。
    ///
    /// 哈希本体在 WGSL 里，这里钉住 CPU 侧镜像的同一公式 —— 两边公式改了
    /// 要同步，漂移会表现为"GPU 测试两遍不一致"。
    #[test]
    fn hash_formula_is_stable() {
        fn cpu_hash(p: [i32; 2]) -> f32 {
            // 与 SSAO_AO_WGSL::hash_angle 逐运算对齐（加 / 异或 / 乘的顺序）。
            let mut h = (p[0] as u32)
                .wrapping_mul(1973)
                .wrapping_add((p[1] as u32).wrapping_mul(9277))
                .wrapping_add(1);
            h = (h ^ (h >> 13)).wrapping_mul(1274126177);
            (h & 0xFFFF) as f32 / 65536.0
        }
        assert_eq!(cpu_hash([3, 7]), cpu_hash([3, 7]));
        assert!(cpu_hash([3, 7]) != cpu_hash([7, 3]), "不同像素应给不同角");
    }
}
