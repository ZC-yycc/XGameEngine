//! 程序化天空盒 + 环境光照（IBL）。
//!
//! # 天空是一个解析函数
//!
//! 天顶 / 地平线 / 地面三段颜色渐变 + 太阳盘 + 太阳光晕，直接对**方向**求值
//! （[`cpu::sky_radiance`] 是它的 CPU 逐式镜像，WGSL 版在 [`SKY_COMMON_WGSL`] 里）。
//! 不做 Preetham/Hosek 那类物理大气散射 —— 前者回答"长什么样"，后者回答
//! "为什么长这样"。先把前者做扎实：同样的出口（一个"方向 → 辐射亮度"的函数）
//! 以后换成物理模型时，下游（天空盒 / IBL / 测试）一行都不用改。
//!
//! # 两个消费者，一个真相源
//!
//! ```text
//!                       ┌─ 天空盒 pass（全屏三角形，场景 pass 内、不透明之后）
//!  sky_radiance(dir) ───┤
//!                       ├─ 漫反射 IBL：CPU 数值积分 L2 球谐（存 E/π）
//!                       └─ 镜面 IBL：GPU GGX 重要性采样 → 预过滤立方体贴图（5 级 mip）
//! ```
//!
//! 背景与照明从同一个函数现取 ⇒ "背景是蓝天、物体却按另一套环境光照明"的
//! 分叉从根上不存在。这是天空比"一张背景图"重要的地方。
//!
//! # 球谐（漫反射环境）
//!
//! 9 个 L2 实球谐系数，CPU 侧用**确定性**的斐波那契球点（1024 个、等权）
//! 数值积分。存的是 **E/π**：余弦卷积因子 `[1, 2/3, 1/4]` 已经折进系数里
//! （着色器端只做"求值"），因此与常量 `ambient` 同一标定 —— 均匀环境下
//! `albedo * eval = albedo * ambient`，两套环境光能直接相加。
//!
//! # 预过滤立方体贴图（镜面环境）
//!
//! 经典 split-sum 的第一项：每个 mip 级按 roughness 做 GGX 重要性采样
//! （Hammersley 序列，无随机数 ⇒ 逐位确定），128² 基、5 级 mip（8² 封顶），
//! 直接对解析天空采样（不需要先渲一张源天空立方体）。第二项（BRDF LUT，
//! 128² Rgba16Float）**首次真正用到时**才生成一次。
//!
//! 立方体六个面的方向约定（`dir = forward + right·(2u-1) + up·(1-2v)`，基表见
//! [`cpu::cube_face_basis`]）与 WebGPU 立方体贴图采样约定逐项对齐 —— 这条
//! 错位的表现是"反射里北边的太阳跑到南边"，像素测试用"粗糙金属上的太阳反光"
//! 专门钉住它。
//!
//! # 生命周期与成本控制
//!
//! 纹理**固定分配**（立方体 + LUT 常驻，合计 < 1 MB），内容按 key 重建：
//! key = [`uniform_key`]（天空 uniform 的字节哈希，太阳方向 / 颜色 / 太阳尺寸
//! 都在内）。key 不变 ⇒ 每帧只有一次 80 字节的 `write_buffer`；key 变了
//! （编辑器调参、灯光转向）才重跑 30 趟小 pass。**不做每帧重建**：重建本身不贵，
//! 贵的是它会逼着全局绑定组换对象（那是每帧路径，见 `scene_pipeline`）。
//!
//! 天空关闭（`SkyConfig::enabled = false`）时：uniform 照写（把开关位写 0，
//! 片元着色器据此整段跳过 IBL 采样），**不重建任何东西** —— 画面与天空上线前
//! 逐位一致，这是"默认关不改变旧项目"的落脚点。

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use xgame_core::{Light, LightKind, SkyConfig};
use xgame_math::glam::Vec3;

/// 环境立方体贴图的 mip 级数（128² → 8²）。
pub const ENV_MIPS: u32 = 5;

/// 环境立方体贴图的基尺寸。
pub const ENV_BASE_SIZE: u32 = 128;

/// BRDF LUT 边长。
pub const BRDF_LUT_SIZE: u32 = 128;

/// L2 实球谐的系数个数。
pub const SH_COEFFS: usize = 9;

/// 球谐投影的采样点数（斐波那契球，等权）。
const SH_SAMPLES: usize = 1024;

/// 各级 mip 的预过滤采样数（镜面反射的方差就在这个旋钮上）。
///
/// 基级给足：roughness 0 时重要性采样退化成"全落在 R 方向"，多采不吃亏且
/// 让太阳盘在镜面反射里不闪烁；高 mip 分辨率低，采样跟着降。
fn prefilter_samples(level: u32) -> u32 {
    match level {
        0 => 96,
        1 => 48,
        2 => 24,
        3 => 16,
        _ => 12,
    }
}

/// 天空 uniform（必须与 WGSL 的 `SkyUniform` 逐字节对齐）。
///
/// 5 个 vec4 = 80 字节，没有隐藏填充：几个标量参数（开关 / 强度 / 太阳尺寸 /
/// 太阳强度）全部塞进各自 vec4 的 w 分量 —— 它们的语义写在字段注释里，
/// 着色器端从同一个结构体读，不会各读各的。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SkyUniform {
    /// xyz = **指向太阳**的单位向量（= 首盏方向光传播方向取反），
    /// w = 天空是否开启（1 / 0，片元着色器据此跳过 IBL 采样）。
    pub sun_dir: [f32; 4],
    /// rgb = 太阳颜色（线性，取首盏方向光的颜色），
    /// a = 太阳强度乘子（没有方向光时为 0 ⇒ 太阳盘与光晕整段不出现）。
    pub sun_color: [f32; 4],
    /// rgb = 天顶色，a = 整体强度乘子。
    pub zenith: [f32; 4],
    /// rgb = 地平线色，a = 太阳盘半径（度）。
    pub horizon: [f32; 4],
    /// rgb = 地面色，a = 预留（写 0）。
    pub ground: [f32; 4],
}

impl SkyUniform {
    /// 从画面设置 + 太阳信息装配。
    ///
    /// `sun` 来自场景灯光（[`sun_from_lights`]）；`None`（没有方向光）时
    /// 太阳强度写 0 —— "没有灯就没有太阳"在数据层就落实了，着色器不必再
    /// 判断第二遍。
    pub fn build(cfg: &SkyConfig, sun: Option<(Vec3, [f32; 3])>) -> Self {
        let (dir, color, sun_intensity) = match sun {
            Some((toward, color)) => (toward.normalize_or_zero(), color, cfg.sun_intensity),
            None => (Vec3::ZERO, [0.0; 3], 0.0),
        };
        Self {
            sun_dir: [
                dir.x,
                dir.y,
                dir.z,
                if cfg.enabled { 1.0 } else { 0.0 },
            ],
            sun_color: [color[0], color[1], color[2], sun_intensity],
            zenith: [cfg.zenith[0], cfg.zenith[1], cfg.zenith[2], cfg.intensity],
            horizon: [cfg.horizon[0], cfg.horizon[1], cfg.horizon[2], cfg.sun_size],
            ground: [cfg.ground[0], cfg.ground[1], cfg.ground[2], 0.0],
        }
    }

    /// 天空是否开启（着色器读的是 `sun_dir.w`）。
    pub fn enabled(&self) -> bool {
        self.sun_dir[3] > 0.5
    }

    /// 指向太阳的方向（未开启 / 无太阳时是零向量）。
    pub fn sun_dir_vec(&self) -> Vec3 {
        Vec3::new(self.sun_dir[0], self.sun_dir[1], self.sun_dir[2])
    }
}

/// 太阳 = 场景里**第一盏** active 的方向光。
///
/// 取 `packed`（渲染管线实际使用的、截断到 8 盏的列表）而不是原始灯光表：
/// 第 9 盏方向光根本照不到场景，让它当"太阳"会造出"背景里有太阳、物体上
/// 没有对应光"的错位。颜色取灯光的 `color`；方向取传播方向的反向
/// （`normalized_direction` 是光传播方向，太阳在它的反侧）。
pub fn sun_from_lights(packed: &[&Light]) -> Option<(Vec3, [f32; 3])> {
    let light = packed
        .iter()
        .find(|l| l.is_active() && l.kind == LightKind::Directional)?;
    let [dx, dy, dz] = light.normalized_direction();
    let toward = Vec3::new(-dx, -dy, -dz).normalize_or_zero();
    Some((toward, light.color))
}

/// 天空 uniform 的字节哈希 —— 环境贴图内容的重建 key。
///
/// 用 FNV-1a 逐字节混：这里只要"变了就一定换 key"，抗碰撞不是诉求。
/// 位模式哈希（不是浮点比较）：`-0.0`/`0.0` 会判成两个 key，代价只是一次
/// 多余的重建，比"改了参数却没重建"安全得多。
pub fn uniform_key(u: &SkyUniform) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytemuck::bytes_of(u) {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

// ---------------------------------------------------------------------------
// WGSL 公共片段
// ---------------------------------------------------------------------------

/// 天空的 WGSL 公共片段：`SkyUniform` / `SkySH` 类型 + `sky_radiance` /
/// `sky_sh_eval` 两个纯函数。
///
/// **刻意不声明任何绑定**：天空盒（自身布局）、场景片元（global 组 7/8 号），
/// 预过滤（自身布局 0 号）三处的绑定号各不相同，公共片段只带类型与函数、
/// uniform 以**参数**传入（结构体按值传参，WGSL 允许），三处各自声明自己的
/// 绑定再调用。这样"公式只有一份"这条纪律不靠人肉同步。
pub const SKY_COMMON_WGSL: &str = r#"
struct SkyUniform {
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    ground: vec4<f32>,
}
struct SkySH { sh: array<vec4<f32>, 9> }

const SKY_PI: f32 = 3.141592653589793;
// 太阳盘的面辐射亮度系数：真实太阳盘远超"白面 1.0"，40 让它色调映射后是
// 一小块近白、并让 Bloom 有东西可抓。
const SKY_SUN_DISK_RADIANCE: f32 = 40.0;
// 太阳周围的光晕（宽而软的近似二次散射）。
const SKY_SUN_GLOW: f32 = 0.35;
// 预过滤环境立方体的最大 mip（= 级数 - 1，与 sky.rs 的 ENV_MIPS 一致）。
const ENV_MAX_LOD: f32 = 4.0;

// 天空辐射亮度（线性 HDR）。与 sky.rs 的 cpu::sky_radiance 必须逐式对应。
//
// 渐变在 dir.y == 0 处连续：天空段与地面段都收敛到地平线色，没有缝合线。
fn sky_radiance(u: SkyUniform, dir: vec3<f32>) -> vec3<f32> {
    let t = clamp(dir.y, -1.0, 1.0);
    let up_t = pow(max(t, 0.0), 0.5);
    let down_t = pow(max(-t, 0.0), 0.5);
    let sky_c = mix(u.horizon.rgb, u.zenith.rgb, up_t);
    let gnd_c = mix(u.horizon.rgb, u.ground.rgb, down_t);
    var c = select(gnd_c, sky_c, t > 0.0);

    // 太阳：盘面（锐，smoothstep 抗锯齿）+ 光晕（宽）。太阳半径在
    // horizon.w（度），太阳强度在 sun_color.a（无方向光时为 0）。
    let cos_ang = clamp(dot(dir, u.sun_dir.xyz), -1.0, 1.0);
    let ang = acos(cos_ang);
    let sun_rad = max(SKY_PI / 180.0 * u.horizon.w, 1e-4);
    let disk = 1.0 - smoothstep(sun_rad * 0.6, sun_rad, ang);
    let glow = SKY_SUN_GLOW * pow(max(cos_ang, 0.0), 16.0);
    c = c + u.sun_color.rgb * u.sun_color.a
        * (SKY_SUN_DISK_RADIANCE * disk + glow);
    return c * u.zenith.a;
}

// E/π 的球谐求值（系数里已折好余弦卷积）；基函数与 sky.rs 的 cpu::sh_basis
// 逐项一致（[Y00, Y1-1(y), Y10(z), Y11(x), Y2-2, Y2-1, Y20, Y21, Y22]）。
fn sky_sh_eval(sh: SkySH, n: vec3<f32>) -> vec3<f32> {
    return sh.sh[0].rgb * 0.282095
         + sh.sh[1].rgb * (0.488603 * n.y)
         + sh.sh[2].rgb * (0.488603 * n.z)
         + sh.sh[3].rgb * (0.488603 * n.x)
         + sh.sh[4].rgb * (1.092548 * n.x * n.y)
         + sh.sh[5].rgb * (1.092548 * n.y * n.z)
         + sh.sh[6].rgb * (0.315392 * (3.0 * n.z * n.z - 1.0))
         + sh.sh[7].rgb * (1.092548 * n.x * n.z)
         + sh.sh[8].rgb * (0.546274 * (n.x * n.x - n.y * n.y));
}
"#;

// ---------------------------------------------------------------------------
// 天空盒着色器
// ---------------------------------------------------------------------------

/// 天空盒顶点着色器：全屏三角形，不读顶点缓冲。
///
/// 片元位置取 z = 1.0（远平面）配 `depth_compare: LessEqual` + 不写深度：
/// 只在"这一像素还没有更近的几何"时作画。三角形顶点超出 NDC（3 那一端）是
/// 标准做法 —— 少了斜接对角线上的像素级接缝问题，也少一次顶点。
const SKY_VERT_SRC: &str = r#"
struct CameraUniform { view_proj: mat4x4<f32>, inv_view_proj: mat4x4<f32>, camera_pos: vec4<f32> }
@group(0) @binding(0) var<uniform> camera: CameraUniform;

struct SkyOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs_sky(@builtin(vertex_index) vi: u32) -> SkyOut {
    // (0,0) (2,0) (0,2) 三个 uv 顶点 → NDC (-1,-1) (3,-1) (-1,3)。
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: SkyOut;
    out.ndc = uv * 2.0 - 1.0;
    out.pos = vec4<f32>(out.ndc, 1.0, 1.0);
    return out;
}
"#;

/// 天空盒片元着色器：由 NDC 反推视线方向，再对解析天空求值。
///
/// 世界方向 = `inv_view_proj · (ndc, 1, 1)`（远平面上的世界点）减去相机位置。
/// 这里不逐像素解 near/far —— 方位角与透视除法无关，远平面上的点与视线
/// 同向（相机在原点前方），归一化之后的差值就是方向本身。
const SKY_FRAG_SRC: &str = r#"
@fragment
fn fs_sky(input: SkyIn) -> @location(0) vec4<f32> {
    let far = camera.inv_view_proj * vec4<f32>(input.ndc, 1.0, 1.0);
    let dir = normalize(far.xyz / far.w - camera.camera_pos.xyz);
    return vec4<f32>(sky_radiance(sky, dir), 1.0);
}
"#;

/// 天空盒片元的输入结构（与 `SKY_VERT_SRC` 的 `SkyOut` 对应）。
///
/// 三段拼装（公共片段 → 本段 → 函数体）的顺序就是最终 WGSL 的书写顺序，
/// 这里把"输入结构 + 天空 uniform 绑定"单独拎出来，是让函数体那段的缩进
/// 与职责跟另外两处着色器保持一致。
const SKY_FRAG_PRELUDE: &str = r#"
// 天空盒与场景共用全局布局的 binding 0（相机）。顶点与片元是两个独立的
// 模块，各自要声明自己读的绑定 —— 少一份就是"no definition in scope"。
struct CameraUniform { view_proj: mat4x4<f32>, inv_view_proj: mat4x4<f32>, camera_pos: vec4<f32> }
@group(0) @binding(0) var<uniform> camera: CameraUniform;

struct SkyIn {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}
@group(0) @binding(7) var<uniform> sky: SkyUniform;
"#;

// ---------------------------------------------------------------------------
// 预过滤 / BRDF LUT 着色器
// ---------------------------------------------------------------------------

/// 预过滤与 LUT 共用的全屏三角形顶点着色器。
///
/// 与天空盒的差别：NDC 里 `y` 翻过一次，让插值出来的 `uv.y` 直接按
/// **纹理 v 方向**（0 = 顶行）走 —— 立方体面方向约定里 `v` 的口径就是这个，
/// 在这里翻比在每处方向计算里翻不容易漏。
const ENV_VERT_SRC: &str = r#"
struct EnvOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_env(@builtin(vertex_index) vi: u32) -> EnvOut {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: EnvOut;
    out.uv = uv;
    out.pos = vec4<f32>(vec2<f32>(uv.x, 1.0 - uv.y) * 2.0 - 1.0, 0.0, 1.0);
    return out;
}
"#;

/// 预过滤参数（每个 mip / 面一份，动态偏移索引）。
///
/// 面的三根基向量由 CPU 给（[`cpu::cube_face_basis`]），着色器不现算
/// cross —— 镜像测试与 GPU 走同一张表，`±Y` 面那两处"up 轴换轴"的分支
/// 就不存在了。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct PrefilterParams {
    /// xyz = 面前向（该面的法坐标轴）。
    forward: [f32; 4],
    /// xyz = 面右向（u 增大方向），w = 该 mip 的 roughness。
    right: [f32; 4],
    /// xyz = 面上向（v 减小方向），w = 采样数。
    up: [f32; 4],
}

/// 预过滤片元着色器（WGSL 源在 [`prefilter_wgsl`] 里跟公共片段拼装）。
const PREFILTER_FS_SRC: &str = r#"
@group(0) @binding(0) var<uniform> sky_uni: SkyUniform;
@group(0) @binding(1) var<uniform> pre: PrefilterParams;

// 位翻转（van der Corput）→ 联合低差异序列的一半。
fn radical_inverse_vdc(bits_in: u32) -> f32 {
    var bits = bits_in;
    bits = (bits << 16u) | (bits >> 16u);
    bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
    bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
    bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
    bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
    return f32(bits) * 2.3283064365386963e-10;
}
fn hammersley(i: u32, n: u32) -> vec2<f32> {
    return vec2<f32>(f32(i) / f32(n), radical_inverse_vdc(i));
}
// GGX 分布的半程向量重要性采样（切空间 z 轴 = n）。
fn importance_sample_ggx(xi: vec2<f32>, n: vec3<f32>, roughness: f32) -> vec3<f32> {
    let a = roughness * roughness;
    let phi = 6.283185307179586 * xi.x;
    let cos_theta = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
    let sin_theta = sqrt(max(1.0 - cos_theta * cos_theta, 0.0));
    let h = vec3<f32>(sin_theta * cos(phi), sin_theta * sin(phi), cos_theta);
    // 切空间 → 世界。n 接近 ±Z 时换一个参考轴，避免 cross 退化。
    let up = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.z) > 0.999);
    let tangent = normalize(cross(up, n));
    let bitangent = cross(n, tangent);
    return normalize(tangent * h.x + bitangent * h.y + n * h.z);
}

@fragment
fn fs_prefilter(input: EnvIn) -> @location(0) vec4<f32> {
    // uv → 方向：u 沿右向量、v 反沿上向量（v = 0 是面的顶行）。
    let n = normalize(pre.forward.xyz
        + pre.right.xyz * (2.0 * input.uv.x - 1.0)
        + pre.up.xyz * (1.0 - 2.0 * input.uv.y));
    let roughness = pre.right.w;
    let samples = u32(pre.up.w);

    // V = N = R（预过滤的是"沿该方向看出去"的积分，标准 split-sum 做法）。
    var acc = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i = 0u; i < samples; i = i + 1u) {
        let xi = hammersley(i, samples);
        let h = importance_sample_ggx(xi, n, roughness);
        let l = normalize(2.0 * dot(n, h) * h - n);
        let n_dot_l = max(dot(n, l), 0.0);
        if (n_dot_l > 0.0) {
            acc = acc + sky_radiance(sky_uni, l) * n_dot_l;
            weight = weight + n_dot_l;
        }
    }
    return vec4<f32>(acc / max(weight, 1e-5), 1.0);
}
"#;

/// BRDF LUT 片元着色器：split-sum 第二项（Karis 积分），无绑定 —— 采样数
/// 是编译期常量。
///
/// LUT 的坐标语义：`x = N·V`，`y = roughness`。场景片元里用
/// `(n_dot_v, roughness)` 采样，返回的 `(A, B)` 满足
/// `镜面环境 = 预过滤色 × (F0 × A + B)`。
const BRDF_FS_SRC: &str = r#"
const BRDF_SAMPLES: u32 = 256u;

fn radical_inverse_vdc(bits_in: u32) -> f32 {
    var bits = bits_in;
    bits = (bits << 16u) | (bits >> 16u);
    bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
    bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
    bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
    bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
    return f32(bits) * 2.3283064365386963e-10;
}
fn hammersley(i: u32, n: u32) -> vec2<f32> {
    return vec2<f32>(f32(i) / f32(n), radical_inverse_vdc(i));
}
fn importance_sample_ggx(xi: vec2<f32>, n: vec3<f32>, roughness: f32) -> vec3<f32> {
    let a = roughness * roughness;
    let phi = 6.283185307179586 * xi.x;
    let cos_theta = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
    let sin_theta = sqrt(max(1.0 - cos_theta * cos_theta, 0.0));
    let h = vec3<f32>(sin_theta * cos(phi), sin_theta * sin(phi), cos_theta);
    let up = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.z) > 0.999);
    let tangent = normalize(cross(up, n));
    let bitangent = cross(n, tangent);
    return normalize(tangent * h.x + bitangent * h.y + n * h.z);
}
// IBL 口径的 Smith 可见性（k = α²/2，与直接光的 k = α/2 不同）。
fn geometry_schlick_ggx(n_dot_x: f32, roughness: f32) -> f32 {
    let k = (roughness * roughness) / 2.0;
    return n_dot_x / (n_dot_x * (1.0 - k) + k);
}

@fragment
fn fs_brdf(input: EnvIn) -> @location(0) vec4<f32> {
    let n_dot_v = clamp(input.uv.x, 0.001, 1.0);
    let roughness = clamp(input.uv.y, 0.001, 1.0);
    // 把法线放在 +Z 的切空间里积分（LUT 与朝向无关，这正是它能预计算的原因）。
    let v = vec3<f32>(sqrt(max(1.0 - n_dot_v * n_dot_v, 0.0)), 0.0, n_dot_v);
    let n = vec3<f32>(0.0, 0.0, 1.0);
    var a = 0.0;
    var b = 0.0;
    for (var i = 0u; i < BRDF_SAMPLES; i = i + 1u) {
        let xi = hammersley(i, BRDF_SAMPLES);
        let h = importance_sample_ggx(xi, n, roughness);
        let l = normalize(2.0 * dot(v, h) * h - v);
        let n_dot_l = max(l.z, 0.0);
        let n_dot_h = max(h.z, 0.0);
        let v_dot_h = max(dot(v, h), 0.0);
        if (n_dot_l > 0.0) {
            let g = geometry_schlick_ggx(n_dot_v, roughness)
                  * geometry_schlick_ggx(n_dot_l, roughness);
            let g_vis = (g * v_dot_h) / (n_dot_h * n_dot_v + 1e-6);
            let fc = pow(1.0 - v_dot_h, 5.0);
            a = a + (1.0 - fc) * g_vis;
            b = b + fc * g_vis;
        }
    }
    return vec4<f32>(a / f32(BRDF_SAMPLES), b / f32(BRDF_SAMPLES), 0.0, 1.0);
}
"#;

/// 预过滤片元的输入结构（`ENV_VERT_SRC` 的 `EnvOut` 对应）。
const ENV_PRELUDE: &str = r#"
struct EnvIn {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}
// 与 sky.rs 的 PrefilterParams 逐字节对齐（每 mip / 面一份，动态偏移索引）。
struct PrefilterParams {
    forward: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
}
"#;

/// 天空盒片元的完整源（公共片段在前：结构体、绑定、函数都能被 fs_sky 看见）。
fn sky_frag_wgsl() -> String {
    format!("{SKY_COMMON_WGSL}{SKY_FRAG_PRELUDE}{SKY_FRAG_SRC}")
}

/// 预过滤片元的完整源。
fn prefilter_frag_wgsl() -> String {
    format!("{SKY_COMMON_WGSL}{ENV_PRELUDE}{PREFILTER_FS_SRC}")
}

/// BRDF LUT 片元的完整源。
fn brdf_frag_wgsl() -> String {
    format!("{ENV_PRELUDE}{BRDF_FS_SRC}")
}

// ---------------------------------------------------------------------------
// GPU 资源
// ---------------------------------------------------------------------------

/// 天空的全部 GPU 资源：全局绑定组要用的四张表 + 两条自己的管线。
///
/// 生命周期挂在 [`crate::scene_pipeline::ScenePipeline`] 上（创建一次、随
/// 全局绑定组同生共死）；这里只关心"内容何时重建"。
pub struct SkyResources {
    /// 天空 uniform（全局组 binding 7，预过滤管线 binding 0 也读它）。
    pub uniform_buffer: wgpu::Buffer,
    /// 球谐系数（全局组 binding 8）。
    pub sh_buffer: wgpu::Buffer,
    /// 预过滤环境立方体（全局组 binding 9 提供 `env_view`）。
    pub env_texture: wgpu::Texture,
    pub env_view: wgpu::TextureView,
    /// 环境采样器（binding 10；立方体与 LUT 共用）。
    pub env_sampler: wgpu::Sampler,
    /// BRDF LUT（全局组 binding 11 提供 `brdf_view`）。
    pub brdf_texture: wgpu::Texture,
    pub brdf_view: wgpu::TextureView,

    /// 天空盒管线（全屏三角形，画进场景 pass）。
    sky_pipeline: wgpu::RenderPipeline,

    /// 预过滤管线与它的动态 uniform（30 槽：6 面 × 5 mip）。
    prefilter_pipeline: wgpu::RenderPipeline,
    prefilter_bind_group: wgpu::BindGroup,
    prefilter_params: wgpu::Buffer,
    /// 预过滤 uniform 的动态偏移步长（设备的最小对齐）。
    uniform_alignment: u64,

    /// BRDF LUT 管线（无绑定组）。
    brdf_pipeline: wgpu::RenderPipeline,

    /// 环境内容对应的 key（[`uniform_key`]）；`None` = 还没建过。
    /// 注意**不因关闭而清空**：关闭只是不采样，内容对同一个 key 仍然有效，
    /// 重新开启且参数没变就免一次重建。
    built_key: Option<u64>,
    /// BRDF LUT 是否已生成（惰性：首次真正开启天空时才跑一次）。
    brdf_built: bool,
}

impl SkyResources {
    /// 建齐所有常驻对象。纹理在这里**一次性分配**（不会随参数变化重建），
    /// 内容为空（WebGPU 资源零初始化 —— 没写过的地方是全 0，永远不会被采样
    /// 到无效显存）。
    pub fn new(
        device: &wgpu::Device,
        global_bgl: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Self {
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sky_uniform"),
            contents: bytemuck::bytes_of(&SkyUniform::build(&SkyConfig::default(), None)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let sh_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sky_sh"),
            contents: bytemuck::cast_slice(&[[0.0f32; 4]; SH_COEFFS]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let env_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sky_env_cube"),
            size: wgpu::Extent3d {
                width: ENV_BASE_SIZE,
                height: ENV_BASE_SIZE,
                depth_or_array_layers: 6,
            },
            mip_level_count: ENV_MIPS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let env_view = env_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("sky_env_cube_view"),
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        });

        let brdf_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sky_brdf_lut"),
            size: wgpu::Extent3d {
                width: BRDF_LUT_SIZE,
                height: BRDF_LUT_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let brdf_view = brdf_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let env_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sky_env_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // 镜面 IBL 靠 mip 插值实现"粗糙度连续" —— 这一项必须是 Linear。
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // -- 天空盒管线：复用全局布局（binding 0 相机 + 7 天空 uniform）--
        let sky_vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky_vs"),
            source: wgpu::ShaderSource::Wgsl(SKY_VERT_SRC.into()),
        });
        let sky_fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky_fs"),
            source: wgpu::ShaderSource::Wgsl(sky_frag_wgsl().into()),
        });
        let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky_pl"),
            bind_group_layouts: &[global_bgl],
            push_constant_ranges: &[],
        });
        let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky_pipeline"),
            layout: Some(&sky_layout),
            vertex: wgpu::VertexState {
                module: &sky_vs,
                entry_point: Some("vs_sky"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &sky_fs,
                entry_point: Some("fs_sky"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                // 不写深度（背景不该挡住任何东西），LessEqual + z = 1.0
                // 让"空像素"通过、任何被几何覆盖的像素失败。
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count.max(1),
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        });

        // -- 预过滤 --
        let prefilter_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky_prefilter_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: std::num::NonZeroU64::new(
                            std::mem::size_of::<PrefilterParams>() as u64,
                        ),
                    },
                    count: None,
                },
            ],
        });
        let uniform_alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let prefilter_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky_prefilter_params"),
            size: 6 * ENV_MIPS as u64 * uniform_alignment,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let prefilter_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky_prefilter_bg"),
            layout: &prefilter_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &prefilter_params,
                        offset: 0,
                        size: std::num::NonZeroU64::new(
                            std::mem::size_of::<PrefilterParams>() as u64,
                        ),
                    }),
                },
            ],
        });

        let env_vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky_env_vs"),
            source: wgpu::ShaderSource::Wgsl(ENV_VERT_SRC.into()),
        });
        let prefilter_fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky_prefilter_fs"),
            source: wgpu::ShaderSource::Wgsl(prefilter_frag_wgsl().into()),
        });
        let prefilter_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky_prefilter_pl"),
            bind_group_layouts: &[&prefilter_bgl],
            push_constant_ranges: &[],
        });
        let prefilter_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky_prefilter_pipeline"),
            layout: Some(&prefilter_layout),
            vertex: wgpu::VertexState {
                module: &env_vs,
                entry_point: Some("vs_env"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &prefilter_fs,
                entry_point: Some("fs_prefilter"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // -- BRDF LUT：无绑定组的全屏 pass --
        let brdf_fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky_brdf_fs"),
            source: wgpu::ShaderSource::Wgsl(brdf_frag_wgsl().into()),
        });
        let brdf_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky_brdf_pl"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });
        let brdf_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky_brdf_pipeline"),
            layout: Some(&brdf_layout),
            vertex: wgpu::VertexState {
                module: &env_vs,
                entry_point: Some("vs_env"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &brdf_fs,
                entry_point: Some("fs_brdf"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            uniform_buffer,
            sh_buffer,
            env_texture,
            env_view,
            env_sampler,
            brdf_texture,
            brdf_view,
            sky_pipeline,
            prefilter_pipeline,
            prefilter_bind_group,
            prefilter_params,
            uniform_alignment,
            brdf_pipeline,
            built_key: None,
            brdf_built: false,
        }
    }

    /// 每帧的入口：写 uniform；内容 key 变了才重建（见模块头）。
    ///
    /// 顺序很重要：`write_buffer` 排在预过滤 pass 之前，两者同属一次提交 ——
    /// wgpu 按队列顺序保证 pass 读到的是新值。
    pub fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform: SkyUniform,
    ) {
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
        if !uniform.enabled() {
            return;
        }
        let key = uniform_key(&uniform);
        if self.built_key == Some(key) {
            return;
        }

        // ① 漫反射：球谐（CPU 侧数值积分，1024 个确定性采样点）。
        let sh = cpu::sh_project(&uniform);
        queue.write_buffer(&self.sh_buffer, 0, bytemuck::cast_slice(&sh[..]));

        // ② 镜面 LUT：首次真正用到才建（关着天空的项目永远不付这笔钱）。
        if !self.brdf_built {
            self.record_brdf_lut(encoder);
            self.brdf_built = true;
        }

        // ③ 预过滤参数（30 槽一次写全），再逐面逐 mip 跑 pass。
        let mut staging = vec![0u8; 6 * ENV_MIPS as usize * self.uniform_alignment as usize];
        for level in 0..ENV_MIPS {
            for face in 0..6usize {
                let (forward, right, up) = cpu::cube_face_basis(face);
                let params = PrefilterParams {
                    forward: [forward.x, forward.y, forward.z, 0.0],
                    right: [
                        right.x,
                        right.y,
                        right.z,
                        level as f32 / (ENV_MIPS - 1) as f32,
                    ],
                    up: [up.x, up.y, up.z, prefilter_samples(level) as f32],
                };
                let offset = (level as usize * 6 + face) * self.uniform_alignment as usize;
                let bytes = bytemuck::bytes_of(&params);
                staging[offset..offset + bytes.len()].copy_from_slice(bytes);
            }
        }
        queue.write_buffer(&self.prefilter_params, 0, &staging);

        for level in 0..ENV_MIPS {
            for face in 0..6u32 {
                let view = self.env_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("sky_env_face"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    base_array_layer: face,
                    array_layer_count: Some(1),
                    ..Default::default()
                });
                let offset = ((level * 6 + face) as u64 * self.uniform_alignment) as u32;
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("sky_prefilter_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    ..Default::default()
                });
                pass.set_pipeline(&self.prefilter_pipeline);
                pass.set_bind_group(0, &self.prefilter_bind_group, &[offset]);
                pass.draw(0..3, 0..1);
            }
        }
        self.built_key = Some(key);
    }

    /// 生成 BRDF LUT 的那一趟 pass。
    fn record_brdf_lut(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sky_brdf_lut_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.brdf_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });
        pass.set_pipeline(&self.brdf_pipeline);
        pass.draw(0..3, 0..1);
    }

    /// 在场景 pass 里画天空盒（不透明之后、半透明之前调用）。
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, global_bind_group: &wgpu::BindGroup) {
        pass.set_pipeline(&self.sky_pipeline);
        // 全局组整帧一份，无动态偏移（实例数据走 storage，天空不读它）。
        pass.set_bind_group(0, global_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// 全局绑定组（group 0）里天空相关的 5 个条目（7..11）。
///
/// 由 `scene_pipeline` 拼进它的全局布局 —— 绑定号是跨着色器契约
/// （天空盒 / 场景片元 / 预过滤三处的声明必须对上），集中在这里声明比散在
/// 两个文件里安全。
pub(crate) fn global_layout_entries() -> [wgpu::BindGroupLayoutEntry; 5] {
    [
        // 7: 天空 uniform（天空盒的顶点不用、片元用；场景片元也用）
        wgpu::BindGroupLayoutEntry {
            binding: 7,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        // 8: 球谐系数（场景片元的漫反射环境）
        wgpu::BindGroupLayoutEntry {
            binding: 8,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        // 9: 预过滤环境立方体
        wgpu::BindGroupLayoutEntry {
            binding: 9,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::Cube,
                multisampled: false,
            },
            count: None,
        },
        // 10: 环境采样器（立方体与 BRDF LUT 共用）
        wgpu::BindGroupLayoutEntry {
            binding: 10,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        // 11: BRDF LUT
        wgpu::BindGroupLayoutEntry {
            binding: 11,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
    ]
}

// ---------------------------------------------------------------------------
// CPU 镜像（与 WGSL 逐式对应；GPU 像素测试的对表依据）
// ---------------------------------------------------------------------------

/// 与 WGSL 同式的 CPU 实现。
///
/// 存在的理由与 `postfx::cpu` 相同：像素测试要**对表**，预期值不该是手抄的
/// 魔数（改一次渐变曲线要人肉重算十几处），而应由这份镜像现算；它同时是
/// WGSL 数值的回归网（着色器里敲错一位，对表测试会与镜像分道扬镳而红）。
///
/// 纪律：这份代码与 [`SKY_COMMON_WGSL`] **必须逐式对应**，改一边就改另一边。
pub mod cpu {
    use super::{SkyUniform, SH_COEFFS, SH_SAMPLES};
    use xgame_math::glam::Vec3;

    /// 与 WGSL `sky_radiance` 逐式对应。
    pub fn sky_radiance(u: &SkyUniform, dir: Vec3) -> [f32; 3] {
        let t = dir.y.clamp(-1.0, 1.0);
        let up_t = t.max(0.0).powf(0.5);
        let down_t = (-t).max(0.0).powf(0.5);
        let mix3 = |a: [f32; 3], b: [f32; 3], k: f32| {
            [
                a[0] + (b[0] - a[0]) * k,
                a[1] + (b[1] - a[1]) * k,
                a[2] + (b[2] - a[2]) * k,
            ]
        };
        let horizon = [u.horizon[0], u.horizon[1], u.horizon[2]];
        let zenith = [u.zenith[0], u.zenith[1], u.zenith[2]];
        let ground = [u.ground[0], u.ground[1], u.ground[2]];
        let mut c = if t > 0.0 {
            mix3(horizon, zenith, up_t)
        } else {
            mix3(horizon, ground, down_t)
        };

        let cos_ang = dir.dot(u.sun_dir_vec()).clamp(-1.0, 1.0);
        let ang = cos_ang.acos();
        let sun_rad = (std::f32::consts::PI / 180.0 * u.horizon[3]).max(1e-4);
        let disk = 1.0 - smoothstep(sun_rad * 0.6, sun_rad, ang);
        let glow = 0.35 * cos_ang.max(0.0).powf(16.0);
        for (c, s) in c.iter_mut().zip(&u.sun_color[..3]) {
            *c += s * u.sun_color[3] * (40.0 * disk + glow);
        }
        c.map(|x| x * u.zenith[3])
    }

    fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// L2 实球谐基（与 WGSL `sky_sh_eval` 的系数逐项一致）。
    ///
    /// 顺序：[Y00, Y1-1(y), Y10(z), Y11(x), Y2-2, Y2-1, Y20, Y21, Y22]。
    pub fn sh_basis(n: Vec3) -> [f32; SH_COEFFS] {
        [
            0.282_095,
            0.488_603 * n.y,
            0.488_603 * n.z,
            0.488_603 * n.x,
            1.092_548 * n.x * n.y,
            1.092_548 * n.y * n.z,
            0.315_392 * (3.0 * n.z * n.z - 1.0),
            1.092_548 * n.x * n.z,
            0.546_274 * (n.x * n.x - n.y * n.y),
        ]
    }

    /// 余弦卷积因子（存 E/π 口径，见模块头）。带阶数：l = 0 / 1 / 2。
    const SH_CONVOLUTION: [f32; 3] = [1.0, 2.0 / 3.0, 0.25];

    fn convolution_of(index: usize) -> f32 {
        SH_CONVOLUTION[if index == 0 {
            0
        } else if index < 4 {
            1
        } else {
            2
        }]
    }

    /// 确定性斐波那契球点（等权积分用，**无随机数** —— 重建结果逐位可复现）。
    pub fn fibonacci_dirs(count: usize) -> Vec<Vec3> {
        let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
        (0..count)
            .map(|i| {
                let y = 1.0 - 2.0 * (i as f32 + 0.5) / count as f32;
                let r = (1.0 - y * y).max(0.0).sqrt();
                let phi = golden * i as f32;
                Vec3::new(r * phi.cos(), y, r * phi.sin())
            })
            .collect()
    }

    /// 把解析天空投影到 L2 球谐，并**把余弦卷积折进系数**（存 E/π）。
    ///
    /// 也就等于：结果的球谐求值 `sh_eval(&coeffs, n)` 直接就是漫反射环境项
    /// `E(n)/π`（与常量 `ambient` 同标定）。
    pub fn sh_project(u: &SkyUniform) -> [[f32; 4]; SH_COEFFS] {
        let dirs = fibonacci_dirs(SH_SAMPLES);
        let weight = 4.0 * std::f32::consts::PI / SH_SAMPLES as f32;
        let mut coeffs = [[0.0f32; 4]; SH_COEFFS];
        for dir in dirs {
            let radiance = sky_radiance(u, dir);
            let basis = sh_basis(dir);
            for (i, b) in basis.iter().enumerate() {
                let k = b * weight * convolution_of(i);
                for ch in 0..3 {
                    coeffs[i][ch] += radiance[ch] * k;
                }
            }
        }
        coeffs
    }

    /// 与 WGSL `sky_sh_eval` 逐式对应。
    pub fn sh_eval(coeffs: &[[f32; 4]; SH_COEFFS], n: Vec3) -> [f32; 3] {
        let basis = sh_basis(n);
        let mut out = [0.0f32; 3];
        for (i, b) in basis.iter().enumerate() {
            for ch in 0..3 {
                out[ch] += coeffs[i][ch] * b;
            }
        }
        out
    }

    /// 立方体 6 个面的基向量：`(forward, right, up)`。
    ///
    /// 面序 = WebGPU 立方体贴图的层序：+X, -X, +Y, -Y, +Z, -Z。方向约定为
    /// `dir = forward + right·(2u-1) + up·(1-2v)`（v = 0 是面的顶行），与
    /// 官方立方体贴图采样约定逐项对齐（+X 面：u 沿 -Z、v 沿 -Y …）。
    pub fn cube_face_basis(face: usize) -> (Vec3, Vec3, Vec3) {
        match face {
            0 => (Vec3::X, -Vec3::Z, Vec3::Y),
            1 => (-Vec3::X, Vec3::Z, Vec3::Y),
            2 => (Vec3::Y, Vec3::X, -Vec3::Z),
            3 => (-Vec3::Y, Vec3::X, Vec3::Z),
            4 => (Vec3::Z, Vec3::X, Vec3::Y),
            _ => (-Vec3::Z, -Vec3::X, Vec3::Y),
        }
    }

    /// 面内归一化坐标 `(u, v)`（v = 0 顶行）→ 方向（已归一化）。
    pub fn face_direction(face: usize, u: f32, v: f32) -> Vec3 {
        let (forward, right, up) = cube_face_basis(face);
        (forward + right * (2.0 * u - 1.0) + up * (1.0 - 2.0 * v)).normalize()
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn daylight() -> SkyConfig {
        SkyConfig {
            enabled: true,
            ..Default::default()
        }
    }

    /// 渐变在地平线处必须连续：视线刚好水平时，从上方与从下方趋近都要收敛到
    /// 地平线色（否则天空与地面之间会出现一条色阶"缝合线"）。
    #[test]
    fn sky_gradient_is_continuous_at_the_horizon() {
        let u = SkyUniform::build(&daylight(), None);
        let eps = 1e-6;
        let up = cpu::sky_radiance(&u, Vec3::new(1.0, eps, 0.0).normalize());
        let down = cpu::sky_radiance(&u, Vec3::new(1.0, -eps, 0.0).normalize());
        let horizon = [u.horizon[0], u.horizon[1], u.horizon[2]];
        for i in 0..3 {
            assert!(
                (up[i] - horizon[i]).abs() < 2e-3 && (down[i] - horizon[i]).abs() < 2e-3,
                "地平线处两段渐变必须都收敛到地平线色: up={up:?} down={down:?} horizon={horizon:?}"
            );
        }
    }

    /// 太阳盘：正对太阳的辐射亮度必须远高于背向；盘外 30° 无盘面贡献；
    /// 没有方向光时太阳整段不出现（数据层就把 sun_intensity 置 0）。
    #[test]
    fn sun_disk_is_local_and_absent_without_a_directional_light() {
        let sun_dir = Vec3::new(0.3, 0.8, 0.5).normalize();
        let lit = SkyUniform::build(&daylight(), Some((sun_dir, [1.0, 0.95, 0.9])));
        let at_sun = cpu::sky_radiance(&lit, sun_dir);
        assert!(
            at_sun[0] > 20.0,
            "太阳盘中心的辐射亮度应当远超白点（面辐射系数 40）: {at_sun:?}"
        );

        // 与太阳成 30° 的方向：盘面半径 1.5° ⇒ 只剩余光滑落 + 渐变。
        let perp = sun_dir.cross(Vec3::Y).normalize();
        let away = (sun_dir * 30f32.to_radians().cos()
            + perp * 30f32.to_radians().sin())
        .normalize();
        let away_r = cpu::sky_radiance(&lit, away);
        for i in 0..3 {
            assert!(
                away_r[i] < 2.0,
                "盘外 30° 不该有盘面贡献: {away_r:?}"
            );
        }

        // 无方向光：太阳强度 0，正对原太阳方向也不亮。天空渐变本身会随方向
        // 变化，所以这里把三段颜色拉平成常量 —— 剩下的任何差异都只可能来自
        // 太阳项（没有方向光时它必须整段消失）。
        let flat = SkyConfig {
            enabled: true,
            zenith: [0.5; 3],
            horizon: [0.5; 3],
            ground: [0.5; 3],
            ..daylight()
        };
        let dark = SkyUniform::build(&flat, None);
        let none_at = cpu::sky_radiance(&dark, sun_dir);
        let none_away = cpu::sky_radiance(&dark, away);
        for i in 0..3 {
            assert!(
                (none_at[i] - none_away[i]).abs() < 1e-4,
                "没有方向光时不该有太阳: {none_at:?} vs {none_away:?}"
            );
        }
    }

    /// 球谐投影的自洽性：**常量天空**投影后再求值必须还是那个常量。
    ///
    /// 这条同时钉住三件事：积分权重（4π/N）、Y00 的归一化（0.282095）、
    /// 以及 l=0 的余弦卷积因子（必须是 1 —— 存的是 E/π 口径）。
    #[test]
    fn sh_projection_of_a_constant_sky_reproduces_the_constant() {
        let cfg = SkyConfig {
            enabled: true,
            intensity: 1.0,
            zenith: [0.5; 3],
            horizon: [0.5; 3],
            ground: [0.5; 3],
            sun_intensity: 0.0,
            ..Default::default()
        };
        let u = SkyUniform::build(&cfg, None);
        let coeffs = cpu::sh_project(&u);
        for dir in [
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::X,
            Vec3::new(0.3, -0.5, 0.8).normalize(),
        ] {
            let v = cpu::sh_eval(&coeffs, dir);
            for (i, c) in v.iter().enumerate() {
                assert!(
                    (c - 0.5).abs() < 0.01,
                    "常量天空的球谐求值必须是原常量（通道 {i}，方向 {dir:?}）: {c}"
                );
            }
        }
    }

    /// 球谐求值必须跟随天空的明暗方向：天顶亮、地面暗 ⇒ 朝上比朝下亮
    /// （哪怕朝向地面的一侧仍有 1/4 的余弦响应）。
    #[test]
    fn sh_diffuse_follows_the_sky_gradient() {
        let cfg = SkyConfig {
            enabled: true,
            zenith: [0.9, 0.9, 0.9],
            horizon: [0.1, 0.1, 0.1],
            ground: [0.0, 0.0, 0.0],
            sun_intensity: 0.0,
            ..Default::default()
        };
        let u = SkyUniform::build(&cfg, None);
        let coeffs = cpu::sh_project(&u);
        let up = cpu::sh_eval(&coeffs, Vec3::Y)[0];
        let down = cpu::sh_eval(&coeffs, Vec3::NEG_Y)[0];
        assert!(
            up > down * 2.0,
            "朝上应当明显比朝下亮: up={up:.3} down={down:.3}"
        );
        assert!(
            up < 0.9 && down > 0.0,
            "环境项是整半球的卷积，不该取到极值: up={up:.3} down={down:.3}"
        );
    }

    /// 立方体面的方向约定：面中心 = 该面法轴；u 增大沿右向、v 减小沿上向；
    /// 以及两个具体的坐标(GPU 采样约定的钉子)：+Z 面右上角在 (+X,+Y,+Z)
    /// 卦限、+Y 面顶行在 -Z 一侧。
    #[test]
    fn cube_face_basis_matches_the_webgpu_convention() {
        for face in 0..6 {
            let (forward, right, up) = cpu::cube_face_basis(face);
            let center = cpu::face_direction(face, 0.5, 0.5);
            assert!(
                center.abs_diff_eq(forward, 1e-6),
                "面 {face} 的中心方向应当是法轴: {center:?} vs {forward:?}"
            );
            let du = (cpu::face_direction(face, 0.6, 0.5) - center).normalize();
            assert!(
                du.dot(right) > 0.99,
                "面 {face}：u 增大应当沿右向"
            );
            let dv = (cpu::face_direction(face, 0.5, 0.4) - center).normalize();
            assert!(dv.dot(up) > 0.99, "面 {face}：v 减小应当沿上向");
            // 三根轴两两正交。
            assert!(right.dot(forward).abs() < 1e-6);
            assert!(up.dot(forward).abs() < 1e-6);
            assert!(right.dot(up).abs() < 1e-6);
        }

        let corner = cpu::face_direction(4, 1.0, 0.0);
        assert!(
            corner.x > 0.5 && corner.y > 0.5 && corner.z > 0.5,
            "+Z 面的右上角应当指向 (+X,+Y,+Z) 卦限: {corner:?}"
        );
        let top_row_plus_y = cpu::face_direction(2, 0.5, 0.0);
        assert!(
            cpu::cube_face_basis(2).2.dot(-Vec3::Z) > 0.99,
            "+Y 面（顶面）的顶行应当在 -Z 一侧: {top_row_plus_y:?}"
        );
    }

    /// key 对每个字段敏感（改了必重建），且投影是确定性的（同输入逐位同结果）。
    #[test]
    fn uniform_key_tracks_every_field_and_projection_is_deterministic() {
        let base_cfg = daylight();
        let sun = Some((Vec3::new(0.2, 0.9, 0.4).normalize(), [1.0, 0.9, 0.8]));
        let base = SkyUniform::build(&base_cfg, sun);
        let base_key = uniform_key(&base);

        let variants: Vec<SkyUniform> = vec![
            SkyUniform::build(&SkyConfig { enabled: false, ..base_cfg }, sun),
            SkyUniform::build(&SkyConfig { intensity: 1.7, ..base_cfg }, sun),
            SkyUniform::build(&SkyConfig { sun_size: 4.0, ..base_cfg }, sun),
            SkyUniform::build(&SkyConfig { sun_intensity: 0.5, ..base_cfg }, sun),
            SkyUniform::build(&SkyConfig { zenith: [0.2, 0.3, 0.5], ..base_cfg }, sun),
            SkyUniform::build(&SkyConfig { horizon: [0.3, 0.4, 0.6], ..base_cfg }, sun),
            SkyUniform::build(&SkyConfig { ground: [0.1, 0.1, 0.1], ..base_cfg }, sun),
            SkyUniform::build(
                &base_cfg,
                Some((Vec3::new(0.1, 0.9, 0.4).normalize(), [1.0, 0.9, 0.8])),
            ),
            SkyUniform::build(&base_cfg, Some((Vec3::Y, [0.9, 0.9, 0.9]))),
            SkyUniform::build(&base_cfg, None),
        ];
        for (i, v) in variants.iter().enumerate() {
            assert_ne!(
                uniform_key(v),
                base_key,
                "第 {i} 个变体改了输入却没改 key —— 环境贴图不会重建"
            );
        }

        // 同输入 → 同 key、同球谐（无随机数）。
        let again = SkyUniform::build(&base_cfg, sun);
        assert_eq!(uniform_key(&again), base_key);
        assert_eq!(cpu::sh_project(&base), cpu::sh_project(&again));
    }

    /// 太阳取场景里**第一盏** active 的方向光：跳过点光、跳过熄灭的灯。
    #[test]
    fn sun_resolves_to_the_first_active_directional_light() {
        let point = Light::point("lamp", [0.0, 3.0, 0.0], [1.0; 3], 2.0, 10.0);
        let dead = Light::directional("dead", [0.0, -1.0, 0.0], [1.0; 3], 0.0);
        let sun = Light::directional("sun", [0.25, -0.5, 0.75], [1.0, 0.9, 0.8], 3.0);
        let second = Light::directional("second", [0.0, -1.0, 0.0], [0.5; 3], 1.0);

        let packed = vec![&point, &dead, &sun, &second];
        let (dir, color) = sun_from_lights(&packed).expect("应当选出一盏方向光");
        let expected = Vec3::new(-0.25, 0.5, -0.75).normalize();
        assert!(
            dir.abs_diff_eq(expected, 1e-5),
            "指向太阳 = 首盏 active 方向光的传播方向取反: {dir:?} vs {expected:?}"
        );
        assert_eq!(color, [1.0, 0.9, 0.8], "颜色取灯光自身颜色");

        // 只有点光 / 全熄灭 → 没有太阳。
        assert!(sun_from_lights(&[&point]).is_none());
        assert!(sun_from_lights(&[&dead]).is_none());
        assert!(sun_from_lights(&[]).is_none());
    }

    /// 开关位就是 uniform 的 `sun_dir.w`：关着时置 0（片元着色器据此跳过
    /// IBL 采样），开着时置 1。
    #[test]
    fn enabled_flag_is_carried_in_the_uniform() {
        let off = SkyUniform::build(&SkyConfig::default(), Some((Vec3::Y, [1.0; 3])));
        assert!(!off.enabled());
        let on = SkyUniform::build(&daylight(), Some((Vec3::Y, [1.0; 3])));
        assert!(on.enabled());
        assert_eq!(on.sun_dir[3], 1.0);
    }
}




