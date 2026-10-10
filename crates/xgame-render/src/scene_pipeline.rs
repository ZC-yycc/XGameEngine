//! 共享 3D 场景渲染管线。
//!
//! 封装 wgpu 渲染管线、着色器、uniform 缓冲区和逐实体绘制逻辑。
//! 可在编辑器离屏渲染器和独立游戏二进制中复用。
//!
//! # 着色模型
//!
//! 采用 **Blinn-Phong**（环境光 + 漫反射 + 半程向量高光），单帧最多
//! [`MAX_LIGHTS`] 盏方向光 / 点光叠加。这不是 PBR——完整 PBR 需要 IBL 与
//! 能量守恒，属于后续"材质系统"的范畴；这一版先把最要紧的三件事做对：
//! **纹理真的被采样**、**高光真的存在**、**多盏灯真的能叠加**。
//!
//! # 绑定组布局
//!
//! ```text
//! group(0)  全局，整帧一份
//!   binding 0  camera    uniform
//!   binding 1  lights    uniform（环境光 + 光源数组）
//!   binding 2  model     uniform（动态偏移：每个实体占一段）
//!   binding 3  shadow    texture_depth_2d（方向光阴影深度图）
//!   binding 4  shadow_smp    comparison sampler
//!   binding 5  shadow_ub     uniform（光源视图投影矩阵 + 偏置/开关）
//!
//! group(1)  材质，每个材质槽一份
//!   binding 0  base color texture
//!   binding 1  sampler
//!   binding 2  material  uniform（反照率因子 + 高光参数）
//! ```
//!
//! 材质绑定组**惰性创建并缓存**（键是 `GltfPrimitive::material_id`），
//! 所以一次模型加载之后，每帧只是查表 + `set_bind_group`，不会重复建对象。
//!
//! # 阴影贴图
//!
//! 方向光走经典的 **shadow mapping**：先用一盏灯（第一盏开了 `casts_shadow`
//! 的方向光）的视图投影矩阵把整场景的深度画进一张独立的深度图，主 pass 里
//! 再把片元变换回光源裁剪空间、与深度图比较（PCF 3×3 软化边缘）。
//!
//! 几个刻意的取舍：
//! - **只做方向光**。点光阴影要 6 面立方体贴图，成本高得多，留到以后。
//! - **只做一盏**。多盏灯各自的阴影贴图意味着每盏一次全场景重绘 + 一张贴图，
//!   收益远不如先把一盏做扎实。第一盏生效，后面的 `casts_shadow` 暂时忽略。
//! - **正交视锥按场景世界包围盒拟合**（[`fit_directional_shadow`]）：阴影贴图是
//!   固定分辨率的，视锥开得太大精度就被稀释。这个矩阵是**纯函数**，因此能单测。
//! - 深度图**常驻一张**（默认 [`DEFAULT_SHADOW_RESOLUTION`]），不随场景变化重建；
//!   灯里的 `shadow_resolution` 改了才会重建（走 [`ScenePipeline::render`] 里那条
//!   "分辨率变了就换图"的分支）。

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use xgame_math::glam::{Mat4, Vec3};

use xgame_core::{Entity, Light, LightKind};
use crate::gltf_loader::{Bounds, GltfMesh, GltfPrimitive, GltfVertex, MaterialParams};

/// 每帧最多渲染的实体数量。
const MAX_RENDER_ENTITIES: usize = 256;

/// 默认阴影贴图分辨率（正方形边长）。
///
/// 2048² 是"看起来干净"和"不吃太多显存"之间一个稳妥的折中：一张
/// Depth32Float 的 2048² 约 16 MB，单次全场景深度重绘也远轻于主 pass。
pub const DEFAULT_SHADOW_RESOLUTION: u32 = 2048;

/// 阴影深度比较的基础偏置（NDC 深度单位）。
///
/// shadow mapping 的经典自遮挡（acne）来自"同一根光线上的表面深度量化误差"，
/// 加一个朝着光源方向的小偏置即可。这个值在拟合出来的正交视锥下约等于
/// 零点几个世界单位 —— 宁可有一点 peter-panning，也不要满屏条纹。
const SHADOW_DEPTH_BIAS: f32 = 0.0022;

/// 单帧支持的最大光源数。
///
/// 这个值在三处必须一致：本常量、[`LightsUniform`] 的数组长度、WGSL 里的
/// `MAX_LIGHTS`。改的时候别漏。
pub const MAX_LIGHTS: usize = 8;

// ---------------------------------------------------------------------------
// GPU uniform 类型（必须与 WGSL 布局逐字节对齐）
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
}

/// 单盏光源的 GPU 表示。
///
/// 方向光和点光共用一个结构体，靠 `pos_type.w` 区分（0=方向光，1=点光）——
/// 这样着色器里只需要一个循环，不必写两套分支逻辑。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct GpuLight {
    /// 方向光：xyz = 光线传播方向，w = 0。
    /// 点光：  xyz = 世界坐标位置，w = 1。
    pos_type: [f32; 4],
    /// rgb = 光色（线性），a = 强度。
    color_intensity: [f32; 4],
    /// x = 这盏灯是否使用阴影贴图（1 / 0），其余预留。
    ///
    /// 只允许**一盏**方向光取 1（见模块头的说明）；多盏同时标记会让它们
    /// 共享同一张深度图，后者的矩阵会覆盖前者，结果是错误但"看起来像阴影"
    /// 的画面 —— 所以在 CPU 侧就只标记第一盏。
    shadow_params: [f32; 4],
}

impl GpuLight {
    const ZERO: Self = Self {
        pos_type: [0.0; 4],
        color_intensity: [0.0; 4],
        shadow_params: [0.0; 4],
    };
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct LightsUniform {
    /// rgb = 环境光，a = 有效光源数量（着色器按它截断循环）。
    ambient: [f32; 4],
    lights: [GpuLight; MAX_LIGHTS],
}

/// 阴影相关的 uniform（主 pass 与阴影 pass 共用同一块缓冲）。
///
/// `params`：x = 一个纹素的 UV 尺寸（PCF 采样间隔），y = 基础深度偏置，
/// z = 阴影是否生效（0 会让片元着色器直接跳过采样），w = 预留。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ShadowUniform {
    light_view_proj: [[f32; 4]; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ModelUniform {
    model: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct MaterialUniform {
    /// rgb = 基础色因子，a = 不透明度。
    base_color: [f32; 4],
    /// x = 高光强度，y = 光泽度（Phong 指数），zw 预留。
    params: [f32; 4],
}

impl MaterialUniform {
    /// 把 glTF 的 metallic-roughness 参数翻译成 Blinn-Phong 的两个经典参数。
    ///
    /// 两处换算都是为了"看起来对"而不是"物理正确"：
    /// - **高光强度**：绝缘体的菲涅尔反射率约 4%，金属接近全反射，所以在
    ///   `0.04 ~ 0.6` 之间按金属度插值（上限压在 0.6，否则全金属模型会白到失形）。
    /// - **光泽度**：`2/r⁴ - 2` 是 Blinn-Phong 指数与 GGX 粗糙度之间的常用近似，
    ///   再夹到 `[2, 512]`——下界防止 `pow(x, 0)` 出现未定义值，上界防止
    ///   高光小到看不见还白白吃算力。
    fn from_params(p: &MaterialParams) -> Self {
        let specular = 0.04 + p.metallic.clamp(0.0, 1.0) * 0.56;
        let r = p.roughness.clamp(0.02, 1.0);
        let shininess = (2.0 / (r * r * r * r) - 2.0).clamp(2.0, 512.0);
        Self {
            base_color: p.base_color,
            params: [specular, shininess, 0.0, 0.0],
        }
    }
}

/// 一帧的光照环境（环境光 + 光源列表）。
///
/// 渲染管线不直接依赖 `Scene`，而是收这个"视图"——编辑器、游戏、以及未来
/// 的预览窗口可以各自组装光照，不必都去构造一个完整的 `Scene`。
#[derive(Clone, Copy, Debug)]
pub struct SceneLighting<'a> {
    /// 全局环境光（线性 RGB）。
    pub ambient: [f32; 3],
    /// 场景光源。超过 [`MAX_LIGHTS`] 的部分会被忽略。
    pub lights: &'a [Light],
}

impl<'a> SceneLighting<'a> {
    pub fn new(ambient: [f32; 3], lights: &'a [Light]) -> Self {
        Self { ambient, lights }
    }
}

// ---------------------------------------------------------------------------
// 阴影拟合（纯函数，便于单测）
// ---------------------------------------------------------------------------

/// 求整场景的**世界空间**包围盒：把所有带模型的实体，按各自的网格本地包围盒
/// 经世界矩阵变换后并起来。
///
/// 变换包围盒只能"变换 8 个角点再重新求盒"——旋转之后对角线方向会变，
/// 不能直接用对角线推。这正是 [`Bounds::transformed`] 做的事。
///
/// 场景里没有任何能画出来的东西时返回 `None`（调用方据此关闭阴影）。
pub fn scene_bounds(
    entities: &[Entity],
    world_matrices: &[Mat4],
    model_cache: &HashMap<String, GltfMesh>,
) -> Option<Bounds> {
    let mut bounds = Bounds::EMPTY;
    for (i, entity) in entities.iter().enumerate() {
        if entity.model.is_empty() {
            continue;
        }
        let Some(mesh) = model_cache.get(&entity.model) else {
            continue;
        };
        let world = world_matrices
            .get(i)
            .copied()
            .unwrap_or_else(|| entity.transform.local_matrix());
        bounds = bounds.union(&mesh.bounds.transformed(&world));
    }
    if bounds.is_empty() {
        None
    } else {
        Some(bounds)
    }
}

/// 选出唯一一盏使用阴影贴图的方向光 —— 在**打包后**的光源列表里的下标。
///
/// `packed` 是"过滤掉强度为 0 的、截断到 [`MAX_LIGHTS`] 之后"的顺序，也就是
/// 着色器看到的数组顺序；标记必须落在同一个下标上，否则会给错误的灯打上阴影。
///
/// 只认方向光：点光阴影要 6 面立方体贴图，不在这一版的范围内。
fn pick_shadow_slot(packed: &[&Light]) -> Option<usize> {
    packed
        .iter()
        .position(|l| l.casts_shadow && l.kind == LightKind::Directional)
}

/// 拟合一盏方向光的正交阴影视锥矩阵（世界 → 光源裁剪空间）。
///
/// # 做法
///
/// 阴影贴图是一张固定分辨率（比如 2048²）的深度图。它只能覆盖一个有限的
/// 立方体区域，所以视锥要**刚好罩住场景**：太大 → 每纹素覆盖的世界尺寸变大，
/// 精度被稀释成一团噪点；太小 → 边缘物体直接没影子。
///
/// 用一个**外接球**定尺寸（球半径 = 包围盒半对角线长）而不是直接用盒：
/// 光源方向是任意角度，球在任何方向上投影都一样大，不用为每个角度重算
/// 包围盒在该方向上的投影范围。代价是比紧致的盒视锥多浪费一些纹素 —— 对
/// "先把阴影做出来"这个阶段是划算的。
///
/// 返回 `None` 表示场景为空或退化，调用方应关闭阴影。
pub fn fit_directional_shadow(direction: Vec3, scene: Bounds) -> Option<Mat4> {
    if scene.is_empty() {
        return None;
    }

    // 零向量方向（数据坏了）退化成"垂直向下"，与 Light::normalized_direction 一致。
    let dir = if direction.length_squared() > 1e-12 {
        direction.normalize()
    } else {
        Vec3::new(0.0, -1.0, 0.0)
    };

    let center = scene.center();
    // 退化场景（只有一个点）时给个下限，否则正交矩阵会塌成一个奇异矩阵。
    let radius = scene.half_extent().length().max(0.5);

    // up 不能与视线平行（look_at 会算出 NaN）。光接近竖直时改用 Z 当上方向。
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };

    let distance = radius * 2.0 + 1.0;
    // 光源在场景中心逆着光的方向退开，视线正对着它 —— 光从"相机"位置照出去。
    let eye = center - dir * distance;
    let view = Mat4::look_at_rh(eye, center, up);

    // 半径留 2% 余量，避免正好贴在边缘的物体被裁掉一角。
    let r = radius * 1.02;
    // near/far 要罩住整个球：最近点距 eye `distance - radius`，最远点 `distance + radius`。
    let near = (distance - radius - 1.0).max(0.01);
    let far = distance + radius + 1.0;
    let proj = Mat4::orthographic_rh(-r, r, -r, r, near, far);
    Some(proj * view)
}

// ---------------------------------------------------------------------------
// 内联 WGSL 着色器
// ---------------------------------------------------------------------------

const SCENE_VERT_SRC: &str = r#"
struct CameraUniform { view_proj: mat4x4<f32>, camera_pos: vec4<f32> }
struct ModelUniform  { model: mat4x4<f32> }

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(0) @binding(2) var<uniform> model: ModelUniform;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) tex_coord: vec2<f32>,
}
struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) tex_coord: vec2<f32>,
}

// 手写 3x3 求逆：non-uniform scale 下法线必须用逆转置矩阵变换，
// 直接乘 model 会让法线歪掉、光照方向全错。
fn mat3_inverse(m: mat3x3<f32>) -> mat3x3<f32> {
    let a = m[0][0]; let b = m[0][1]; let c = m[0][2];
    let d = m[1][0]; let e = m[1][1]; let f = m[1][2];
    let g = m[2][0]; let h = m[2][1]; let i = m[2][2];
    let det = a * (e*i - f*h) - b * (d*i - f*g) + c * (d*h - e*g);
    let inv_det = 1.0 / det;
    return mat3x3<f32>(
        (e*i - f*h)*inv_det, (c*h - b*i)*inv_det, (b*f - c*e)*inv_det,
        (f*g - d*i)*inv_det, (a*i - c*g)*inv_det, (c*d - a*f)*inv_det,
        (d*h - e*g)*inv_det, (b*g - a*h)*inv_det, (a*e - b*d)*inv_det,
    );
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let world_pos = model.model * vec4<f32>(input.position, 1.0);
    let m = model.model;
    let m33 = mat3x3<f32>(m[0].xyz, m[1].xyz, m[2].xyz);
    let normal_mat = transpose(mat3_inverse(m33));
    var out: VertexOutput;
    out.clip_pos = camera.view_proj * world_pos;
    out.world_normal = normal_mat * input.normal;
    out.world_pos = world_pos.xyz;
    out.color = input.color;
    out.tex_coord = input.tex_coord;
    return out;
}
"#;

const SCENE_FRAG_SRC: &str = r#"
const MAX_LIGHTS: u32 = 8u;

struct CameraUniform { view_proj: mat4x4<f32>, camera_pos: vec4<f32> }
struct GpuLight      { pos_type: vec4<f32>, color_intensity: vec4<f32>, shadow_params: vec4<f32> }
struct LightsUniform { ambient: vec4<f32>, lights: array<GpuLight, 8> }
struct MaterialUniform { base_color: vec4<f32>, params: vec4<f32> }
struct ShadowUniform { light_view_proj: mat4x4<f32>, params: vec4<f32> }

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(0) @binding(1) var<uniform> lights: LightsUniform;
@group(0) @binding(3) var shadow_tex: texture_depth_2d;
@group(0) @binding(4) var shadow_smp: sampler_comparison;
@group(0) @binding(5) var<uniform> shadow: ShadowUniform;
@group(1) @binding(0) var base_color_tex: texture_2d<f32>;
@group(1) @binding(1) var base_color_smp: sampler;
@group(1) @binding(2) var<uniform> material: MaterialUniform;

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) tex_coord: vec2<f32>,
}

// 3x3 PCF：把 9 个比较结果平均一下，边缘不那么硬。
//
// 用 `textureSampleCompareLevel` 而不是 `textureSampleCompare`——后者要走隐式
// 导数，而它处在一个"按灯循环里的 if 分支"中（非一致控制流），WGSL 的一致性
// 分析会因此报错。深度图没有 mip，本来也只需要 level 0，正好。
fn shadow_factor(world_pos: vec3<f32>, ndotl: f32) -> f32 {
    let light_clip = shadow.light_view_proj * vec4<f32>(world_pos, 1.0);
    let ndc = light_clip.xyz / light_clip.w;
    // NDC → 纹理 UV。y 要翻：NDC 的 y 朝上，纹理的 v 朝下。
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    // 落出光源视锥（含近平面之后）的部分当作"完全受光"，否则边缘会被裁出一条黑带。
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0;
    }
    // 掠射角下采样点跨度大、自遮挡更严重，所以偏置随 ndotl 变小而放大。
    let bias = shadow.params.y * (1.0 + 2.0 * (1.0 - ndotl));
    let depth = ndc.z;
    var sum = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * shadow.params.x;
            sum = sum + textureSampleCompareLevel(shadow_tex, shadow_smp, uv + offset, depth - bias);
        }
    }
    return sum / 9.0;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // 反照率 = 贴图 × 材质因子 × 顶点色（三者都是"乘法叠加"的直觉语义）
    let sampled = textureSample(base_color_tex, base_color_smp, input.tex_coord);
    let albedo = sampled.rgb * material.base_color.rgb * input.color;
    let alpha = sampled.a * material.base_color.a;

    let N = normalize(input.world_normal);
    let V = normalize(camera.camera_pos.xyz - input.world_pos);

    var color = lights.ambient.rgb;
    let count = min(u32(lights.ambient.a), MAX_LIGHTS);
    let shadows_on = shadow.params.z > 0.5;

    for (var i = 0u; i < MAX_LIGHTS; i = i + 1u) {
        if (i >= count) { break; }
        let light = lights.lights[i];

        var L: vec3<f32>;
        var atten = 1.0;
        if (light.pos_type.w < 0.5) {
            // 方向光：pos_type.xyz 是光线传播方向，入射方向取反
            L = normalize(-light.pos_type.xyz);
        } else {
            // 点光：按到光源的距离做平滑衰减（1/(1+kd+kd²) 的标准近似）
            let to_light = light.pos_type.xyz - input.world_pos;
            let dist = length(to_light);
            L = to_light / max(dist, 1e-4);
            atten = 1.0 / (1.0 + 0.09 * dist + 0.032 * dist * dist);
        }

        let ndotl = max(dot(N, L), 0.0);
        let radiance = light.color_intensity.rgb * light.color_intensity.a * atten;

        // 只有被标记的那盏灯查阴影图；其余灯（含未受光的背光面）跳过采样。
        var shade = 1.0;
        if (shadows_on && light.shadow_params.x > 0.5 && ndotl > 0.0) {
            shade = shadow_factor(input.world_pos, ndotl);
        }

        // Blinn-Phong 高光：用半程向量 H 代替反射向量，比 Phong 更快也更稳
        let H = normalize(L + V);
        let spec = pow(max(dot(N, H), 0.0), material.params.y) * material.params.x * ndotl;

        color = color + (albedo * ndotl + spec) * radiance * shade;
    }

    return vec4<f32>(color, alpha);
}
"#;

/// 阴影 pass 的顶点着色器：只把顶点变换到光源裁剪空间，片元阶段不需要——
/// 深度图只关心深度，管线里 `fragment: None`。
const SHADOW_VERT_SRC: &str = r#"
struct ShadowUniform { light_view_proj: mat4x4<f32>, params: vec4<f32> }
struct ModelUniform  { model: mat4x4<f32> }

@group(0) @binding(0) var<uniform> shadow: ShadowUniform;
@group(0) @binding(1) var<uniform> model: ModelUniform;

@vertex
fn vs_shadow(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    let world_pos = model.model * vec4<f32>(position, 1.0);
    return shadow.light_view_proj * world_pos;
}
"#;

// ---------------------------------------------------------------------------
// ScenePipeline
// ---------------------------------------------------------------------------

/// 一个材质槽的 GPU 资源。
struct MaterialBinding {
    bind_group: wgpu::BindGroup,
    /// 材质 uniform 缓冲。`BindGroup` 内部其实已经持有引用，这里显式留着是为了
    /// 语义清晰——删了这个槽就一起释放。
    _uniform: wgpu::Buffer,
}

/// 共享 3D 场景渲染管线。
///
/// 创建一次，每帧调用 [`render`](Self::render)。不持有离屏纹理或交换链——
/// 由调用者传入。
pub struct ScenePipeline {
    /// 不透明变体。
    pipeline: wgpu::RenderPipeline,
    /// 半透明变体：开启 alpha 混合、关闭深度写入（否则半透明物体会
    /// 把它后面的东西提前挡掉）。
    blend_pipeline: wgpu::RenderPipeline,

    /// 全局绑定组布局。构造完 `global_bind_group` 后就不再被直接使用，
    /// 但显式持有它，免得依赖"wgpu 内部 Arc 一定活着"这种隐含前提。
    _global_bgl: wgpu::BindGroupLayout,
    material_bgl: wgpu::BindGroupLayout,

    camera_buffer: wgpu::Buffer,
    lights_buffer: wgpu::Buffer,
    model_buffer: wgpu::Buffer,
    global_bind_group: wgpu::BindGroup,

    // -- 阴影 --
    /// 阴影 pass 的布局：binding 0 = 阴影 uniform，binding 1 = 模型矩阵（动态偏移）。
    ///
    /// 刻意与 `global_bgl` **分开**：阴影 pass 一边把深度画进 `shadow_view`、
    /// 一边又要在 group(0) 里绑这张图 —— wgpu 会把"同一资源既当渲染目标又当
    /// 绑定资源"判成冲突。用一个不含深度贴图的小布局绕开它。
    ///
    /// 建完管线与绑定组之后就不再被直接使用，冠以 `_` 只是为了表明"留着是为了
    /// 语义清晰，不是被谁读"（与 `_global_bgl` 同理）。
    _shadow_bgl: wgpu::BindGroupLayout,
    shadow_pipeline: wgpu::RenderPipeline,
    /// 阴影 uniform（光源视图投影 + 偏置）。主 pass 与阴影 pass 都读它。
    shadow_buffer: wgpu::Buffer,
    /// 阴影 pass 用的绑定组（阴影 uniform + 模型段）。模型缓冲固定不变，建一次即可。
    shadow_pass_bind_group: wgpu::BindGroup,
    /// 深度比较采样器（PCF）。
    shadow_sampler: wgpu::Sampler,
    /// 阴影深度图。分辨率变化时重建（连同 `global_bind_group`）。
    shadow_texture: wgpu::Texture,
    shadow_view: wgpu::TextureView,
    shadow_resolution: u32,

    /// `material_id` → 材质槽 GPU 资源。
    material_cache: HashMap<u64, MaterialBinding>,

    uniform_alignment: u64,
}

impl ScenePipeline {
    /// 创建渲染管线、绑定组和 uniform 缓冲。
    ///
    /// `format` 是颜色附件的纹理格式（编辑器离屏或交换链）。
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let global_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene_global_bgl"),
            entries: &[
                // 0: 相机（顶点阶段要 view_proj，片元阶段要相机位置算半程向量）
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 1: 光照
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 2: 模型矩阵（动态偏移：一个实体一段，避免每个实体一个绑定组）
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 3: 阴影深度图（片元阶段比较采样）
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        // 深度纹理只能用 Depth 采样类型（不是 Float）
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // 4: 深度比较采样器
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                // 5: 阴影 uniform（光源视图投影 + 偏置 / 开关）
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
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

        let material_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene_material_bgl"),
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

        // 阴影 pass 的布局：故意不含深度贴图本身（见结构体上 shadow_bgl 的注释）。
        let shadow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene_shadow_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_camera_ub"),
            contents: bytemuck::cast_slice(&[CameraUniform {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                camera_pos: [0.0; 4],
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_lights_ub"),
            contents: bytemuck::bytes_of(&LightsUniform {
                ambient: [0.06, 0.06, 0.08, 1.0],
                lights: [GpuLight::ZERO; MAX_LIGHTS],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let model_buffer_size = MAX_RENDER_ENTITIES as u64 * uniform_alignment;
        let model_buffer_init = vec![0u8; model_buffer_size as usize];
        let model_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_model_ub"),
            contents: &model_buffer_init,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // -- 阴影资源 --
        let shadow_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_shadow_ub"),
            contents: bytemuck::bytes_of(&ShadowUniform {
                light_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                // z = 0：默认阴影关闭。没有标记任何灯时片元着色器不采样这张图。
                params: [1.0 / DEFAULT_SHADOW_RESOLUTION as f32, SHADOW_DEPTH_BIAS, 0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene_shadow_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // `Comparison` 采样器配 `textureSampleCompareLevel` 用：
            // 硬件替我们做"片元深度 vs 存图深度"的比较，省掉一次纹理读取 + 分支。
            compare: Some(wgpu::CompareFunction::Less),
            ..Default::default()
        });

        let shadow_resolution = DEFAULT_SHADOW_RESOLUTION;
        let (shadow_texture, shadow_view) = Self::create_shadow_target(device, shadow_resolution);

        let global_bind_group = Self::create_global_bind_group(
            device,
            &global_bgl,
            &camera_buffer,
            &lights_buffer,
            &model_buffer,
            &shadow_view,
            &shadow_sampler,
            &shadow_buffer,
        );

        let shadow_pass_bind_group = Self::create_shadow_pass_bind_group(
            device,
            &shadow_bgl,
            &shadow_buffer,
            &model_buffer,
        );

        let pipeline = Self::build_pipeline(device, &global_bgl, &material_bgl, format, false);
        let blend_pipeline = Self::build_pipeline(device, &global_bgl, &material_bgl, format, true);
        let shadow_pipeline = Self::build_shadow_pipeline(device, &shadow_bgl);

        Self {
            pipeline,
            blend_pipeline,
            _global_bgl: global_bgl,
            material_bgl,
            camera_buffer,
            lights_buffer,
            model_buffer,
            global_bind_group,
            _shadow_bgl: shadow_bgl,
            shadow_pipeline,
            shadow_buffer,
            shadow_pass_bind_group,
            shadow_sampler,
            shadow_texture,
            shadow_view,
            shadow_resolution,
            material_cache: HashMap::new(),
            uniform_alignment,
        }
    }

    /// 创建一张阴影深度图（深度仅用作渲染目标 + 采样源）。
    fn create_shadow_target(device: &wgpu::Device, resolution: u32) -> (wgpu::Texture, wgpu::TextureView) {
        let size = wgpu::Extent3d {
            width: resolution.max(1),
            height: resolution.max(1),
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene_shadow_depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    fn model_binding(buffer: &wgpu::Buffer) -> wgpu::BindingResource<'_> {
        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer,
            offset: 0,
            size: std::num::NonZeroU64::new(std::mem::size_of::<ModelUniform>() as u64),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn create_global_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        camera_buffer: &wgpu::Buffer,
        lights_buffer: &wgpu::Buffer,
        model_buffer: &wgpu::Buffer,
        shadow_view: &wgpu::TextureView,
        shadow_sampler: &wgpu::Sampler,
        shadow_buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene_global_bg"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: lights_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: Self::model_binding(model_buffer) },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(shadow_sampler),
                },
                wgpu::BindGroupEntry { binding: 5, resource: shadow_buffer.as_entire_binding() },
            ],
        })
    }

    fn create_shadow_pass_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        shadow_buffer: &wgpu::Buffer,
        model_buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene_shadow_pass_bg"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: shadow_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: Self::model_binding(model_buffer) },
            ],
        })
    }

    /// 若目标分辨率与当前不一致，重建阴影深度图并更新依赖它的全局绑定组。
    ///
    /// 只在灯里的 `shadow_resolution` 真的变了时才发生；正常一帧是零开销的
    /// 一次整数比较。重建必须连 `global_bind_group` 一起换 —— 它内部持有
    /// 旧的 `TextureView`。
    fn ensure_shadow_resolution(&mut self, device: &wgpu::Device, resolution: u32) {
        let resolution = resolution.max(1);
        if resolution == self.shadow_resolution {
            return;
        }
        let (texture, view) = Self::create_shadow_target(device, resolution);
        self.shadow_texture = texture;
        self.shadow_view = view;
        self.shadow_resolution = resolution;
        self.global_bind_group = Self::create_global_bind_group(
            device,
            &self._global_bgl,
            &self.camera_buffer,
            &self.lights_buffer,
            &self.model_buffer,
            &self.shadow_view,
            &self.shadow_sampler,
            &self.shadow_buffer,
        );
    }

    fn build_pipeline(
        device: &wgpu::Device,
        global_bgl: &wgpu::BindGroupLayout,
        material_bgl: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        blend: bool,
    ) -> wgpu::RenderPipeline {
        let vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene_vs"),
            source: wgpu::ShaderSource::Wgsl(SCENE_VERT_SRC.into()),
        });
        let fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene_fs"),
            source: wgpu::ShaderSource::Wgsl(SCENE_FRAG_SRC.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene_pl"),
            bind_group_layouts: &[global_bgl, material_bgl],
            push_constant_ranges: &[],
        });

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(if blend { "scene_pipeline_blend" } else { "scene_pipeline" }),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vs,
                entry_point: Some("vs_main"),
                buffers: &[GltfVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fs,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    // 不透明用 REPLACE；半透明用标准的 src-alpha / 1-src-alpha 混合
                    blend: Some(if blend {
                        wgpu::BlendState::ALPHA_BLENDING
                    } else {
                        wgpu::BlendState::REPLACE
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                // 半透明物体关掉背面剔除——否则看穿玻璃杯时会缺一半内壁
                cull_mode: if blend { None } else { Some(wgpu::Face::Back) },
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: !blend,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        })
    }

    /// 阴影 pass 的管线：**只有顶点阶段**（无片元），深度写入开、背面剔除。
    ///
    /// 无片元的管线只能配 `color_attachments: &[]`，正是"纯深度预渲染"要的形态。
    /// 剔除背面（而非正面）：正面剔除能缓解 peter-panning，但会让单面几何
    /// （内置 Plane 就只有一个面）从光的方向看过去整个消失、投不出影子。
    /// 对"开箱即用的图元"来说，看得见影子比少一点 peter-panning 重要。
    fn build_shadow_pipeline(device: &wgpu::Device, shadow_bgl: &wgpu::BindGroupLayout) -> wgpu::RenderPipeline {
        let vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene_shadow_vs"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_VERT_SRC.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene_shadow_pl"),
            bind_group_layouts: &[shadow_bgl],
            push_constant_ranges: &[],
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene_shadow_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vs,
                entry_point: Some("vs_shadow"),
                buffers: &[GltfVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: None,
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                // 顺手用一下硬件的深度偏置：斜率相关的那一份交给它，
                // 着色器里只再补一个常数偏置（PCF 采样点的跨度靠那句补）。
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        })
    }

    /// 把**已打包**的光源列表编成 GPU 可用的 uniform。
    ///
    /// `packed` 由调用方按"过滤掉强度为 0 的、截断到 [`MAX_LIGHTS`]"的顺序备好；
    /// `shadow_slot` 是其中那盏使用阴影贴图的方向光的下标
    /// （由 [`pick_shadow_slot`] 决定）。只有它会打上 `shadow_params.x = 1`。
    fn build_lights(ambient: [f32; 3], packed: &[&Light], shadow_slot: Option<usize>) -> LightsUniform {
        let mut uniform = LightsUniform {
            ambient: [ambient[0], ambient[1], ambient[2], 0.0],
            lights: [GpuLight::ZERO; MAX_LIGHTS],
        };

        let mut count = 0usize;
        for light in packed {
            let color = [
                light.color[0],
                light.color[1],
                light.color[2],
                light.intensity,
            ];
            uniform.lights[count] = match light.kind {
                LightKind::Directional => {
                    let d = light.normalized_direction();
                    GpuLight {
                        pos_type: [d[0], d[1], d[2], 0.0],
                        color_intensity: color,
                        shadow_params: [0.0; 4],
                    }
                }
                LightKind::Point => GpuLight {
                    pos_type: [light.position[0], light.position[1], light.position[2], 1.0],
                    color_intensity: color,
                    shadow_params: [0.0; 4],
                },
            };
            count += 1;
        }

        if let Some(slot) = shadow_slot {
            if slot < count {
                uniform.lights[slot].shadow_params[0] = 1.0;
            }
        }

        uniform.ambient[3] = count as f32;
        uniform
    }

    /// 为一个材质槽创建（或复用）绑定组。
    ///
    /// 惰性创建：第一次遇到某个 `material_id` 时建，之后整帧乃至整局复用。
    /// 这也是为什么材质 ID 要全局唯一——不同模型里即使材质完全一样，
    /// 各自持有自己的槽位，不会因为缓存键相同而串台。
    fn material_binding(&mut self, device: &wgpu::Device, prim: &GltfPrimitive) -> &wgpu::BindGroup {
        if !self.material_cache.contains_key(&prim.material_id) {
            let uniform = MaterialUniform::from_params(&prim.material);
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("scene_material_ub"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            // 这里对 `self.material_bgl`（不可变）与 `self.material_cache`（可变）的
            // 借用不重叠——layout 的借用只在 create_bind_group 这一句里活着，
            // 返回的 BindGroup 持有的是自己的资源引用，不挂在 self 上。
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("scene_material_bg"),
                layout: &self.material_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&prim.texture.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&prim.texture.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buffer.as_entire_binding(),
                    },
                ],
            });
            self.material_cache
                .insert(prim.material_id, MaterialBinding { bind_group, _uniform: buffer });
        }
        &self.material_cache[&prim.material_id].bind_group
    }

    /// 执行一帧场景渲染。
    ///
    /// 写入 camera / lights / model uniform，按"先不透明、后半透明（远→近）"
    /// 的顺序发出绘制调用。调用者需提前用 `queue.submit()` 提交 `encoder`。
    ///
    /// # 关于 `world_matrices`
    ///
    /// 着色器要的是**世界矩阵**，而实体自己存的是**局部变换**（层级在
    /// `xgame_core` 里维护）。层级解算不放在渲染器里，是为了让"世界矩阵怎么算"
    /// 只有一份实现（`Scene::world_matrices`），渲染器只消费结果。
    /// 调用方传入的 `world_matrices` 必须与 `entities` 同序同长；长度不足的
    /// 位置退回该实体的局部矩阵，不会 panic。
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        entities: &[Entity],
        world_matrices: &[Mat4],
        model_cache: &HashMap<String, GltfMesh>,
        view_proj: &Mat4,
        camera_pos: &Vec3,
        lighting: SceneLighting<'_>,
    ) {
        // --- 光源打包 + 阴影准备（都在 CPU 侧，先算完再开 pass）---
        let packed: Vec<&Light> = lighting
            .lights
            .iter()
            .filter(|l| l.is_active())
            .take(MAX_LIGHTS)
            .collect();
        let shadow_slot = pick_shadow_slot(&packed);

        // 阴影视锥按场景世界包围盒拟合；取不到包围盒（空场景）就没有阴影。
        let shadow_matrix = match shadow_slot {
            Some(slot) => scene_bounds(entities, world_matrices, model_cache).and_then(|bounds| {
                let light = packed[slot];
                fit_directional_shadow(Vec3::from(light.normalized_direction()), bounds)
            }),
            None => None,
        };
        let shadow_enabled = shadow_matrix.is_some();

        // 分辨率取自那盏灯（0 = 用默认值）。变了才重建深度图。
        if let Some(slot) = shadow_slot {
            let res = match packed[slot].shadow_resolution {
                0 => DEFAULT_SHADOW_RESOLUTION,
                r => r,
            };
            self.ensure_shadow_resolution(device, res);
        }

        // --- 每帧全局 uniform ---
        queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&[CameraUniform {
                view_proj: view_proj.to_cols_array_2d(),
                camera_pos: [camera_pos.x, camera_pos.y, camera_pos.z, 1.0],
            }]),
        );
        queue.write_buffer(
            &self.lights_buffer,
            0,
            bytemuck::bytes_of(&Self::build_lights(lighting.ambient, &packed, shadow_slot)),
        );
        queue.write_buffer(
            &self.shadow_buffer,
            0,
            bytemuck::bytes_of(&ShadowUniform {
                light_view_proj: shadow_matrix
                    .unwrap_or(Mat4::IDENTITY)
                    .to_cols_array_2d(),
                params: [
                    1.0 / self.shadow_resolution as f32,
                    SHADOW_DEPTH_BIAS,
                    if shadow_enabled { 1.0 } else { 0.0 },
                    0.0,
                ],
            }),
        );

        // --- 收集绘制项，同时写 model uniform ---
        // 不透明与半透明分开收集：半透明要按到相机的距离从远到近排序，
        // 否则两个半透明物体前后叠在一起时，谁的 alpha 生效取决于绘制顺序。
        let mut opaque: Vec<(usize, &GltfPrimitive)> = Vec::new();
        let mut blended: Vec<(f32, usize, &GltfPrimitive)> = Vec::new();

        for (i, entity) in entities.iter().enumerate() {
            if i >= MAX_RENDER_ENTITIES {
                break;
            }
            if entity.model.is_empty() {
                continue;
            }
            let Some(mesh) = model_cache.get(&entity.model) else {
                continue;
            };

            // 世界矩阵由调用方解算好（层级在 core 里维护）；越界时退回局部矩阵，
            // 保证即使调用方少传了一段也不会画出诡异的东西。
            let world = world_matrices
                .get(i)
                .copied()
                .unwrap_or_else(|| entity.transform.local_matrix());

            let offset = i as u64 * self.uniform_alignment;
            queue.write_buffer(
                &self.model_buffer,
                offset,
                bytemuck::cast_slice(&[ModelUniform {
                    model: world.to_cols_array_2d(),
                }]),
            );

            // 半透明排序用的是**世界坐标**——实体被父节点挪过位置之后，
            // 局部坐标早就不是它在屏幕上的深度了。
            let world_pos = Vec3::new(world.w_axis.x, world.w_axis.y, world.w_axis.z);
            let dist = (world_pos - *camera_pos).length_squared();
            for prim in &mesh.primitives {
                if prim.material.blend {
                    blended.push((dist, i, prim));
                } else {
                    opaque.push((i, prim));
                }
            }
        }

        // 远 → 近。`partial_cmp` 对 NaN 返回 None，退化成"保持原序"即可，
        // 不值得为一个坏变换把整帧干掉。
        blended.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        // --- 阴影 pass：先把深度画进阴影图（必须在主 pass 之前）---
        //
        // 只画**不透明**图元。半透明物体（玻璃之类）本来也不该投出实心影子，
        // 而且它们不写主深度缓冲，混到阴影图里只会投出一块错误的黑斑。
        if shadow_enabled {
            let mut spass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow_pass"),
                // 深度-only 管线：没有颜色附件。
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations {
                        // 1.0 = 最远。没被任何东西遮住的区域保持"最远"，比较时永远判为受光。
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            spass.set_pipeline(&self.shadow_pipeline);
            for (i, prim) in &opaque {
                let offset = *i as u64 * self.uniform_alignment;
                spass.set_bind_group(0, &self.shadow_pass_bind_group, &[offset as u32]);
                spass.set_vertex_buffer(0, prim.vertex_buffer.slice(..));
                spass.set_index_buffer(prim.index_buffer.slice(..), prim.index_format);
                spass.draw_indexed(0..prim.num_indices, 0, 0..1);
            }
        }

        // --- 渲染通道 ---
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.12, g: 0.12, b: 0.14, a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });

        // 不透明：写深度
        rpass.set_pipeline(&self.pipeline);
        for (i, prim) in &opaque {
            let offset = *i as u64 * self.uniform_alignment;
            rpass.set_bind_group(0, &self.global_bind_group, &[offset as u32]);
            let material = self.material_binding(device, prim);
            rpass.set_bind_group(1, material, &[]);
            rpass.set_vertex_buffer(0, prim.vertex_buffer.slice(..));
            rpass.set_index_buffer(prim.index_buffer.slice(..), prim.index_format);
            rpass.draw_indexed(0..prim.num_indices, 0, 0..1);
        }

        // 半透明：不写深度，远→近
        if !blended.is_empty() {
            rpass.set_pipeline(&self.blend_pipeline);
            for (_, i, prim) in &blended {
                let offset = *i as u64 * self.uniform_alignment;
                rpass.set_bind_group(0, &self.global_bind_group, &[offset as u32]);
                let material = self.material_binding(device, prim);
                rpass.set_bind_group(1, material, &[]);
                rpass.set_vertex_buffer(0, prim.vertex_buffer.slice(..));
                rpass.set_index_buffer(prim.index_buffer.slice(..), prim.index_format);
                rpass.draw_indexed(0..prim.num_indices, 0, 0..1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(m: &Mat4, p: Vec3) -> Vec3 {
        let clip = *m * p.extend(1.0);
        clip.truncate() / clip.w
    }

    // -- Bounds --

    #[test]
    fn empty_bounds_is_empty_and_union_brings_it_to_life() {
        let empty = Bounds::EMPTY;
        assert!(empty.is_empty());

        let one = Bounds::from_points([[1.0, 2.0, 3.0]]);
        assert!(!one.is_empty());
        // 单点盒：min == max。
        assert_eq!(one.min, one.max);

        let merged = empty.union(&one);
        assert_eq!(merged, one, "空盒并任何东西应该得到那个东西本身");

        let two = Bounds::from_points([[-1.0, 0.0, 0.0], [2.0, 5.0, 1.0]]);
        let all = one.union(&two);
        assert_eq!(all.min, Vec3::new(-1.0, 0.0, 0.0));
        assert_eq!(all.max, Vec3::new(2.0, 5.0, 3.0));
    }

    #[test]
    fn bounds_center_and_radius() {
        let b = Bounds {
            min: Vec3::new(-1.0, -1.0, -1.0),
            max: Vec3::new(1.0, 1.0, 1.0),
        };
        assert_eq!(b.center(), Vec3::ZERO);
        assert!((b.radius() - 3f32.sqrt()).abs() < 1e-5);
    }

    #[test]
    fn transformed_bounds_wraps_rotated_corners() {
        // 单位盒绕 Y 轴转 45°，角点会伸到 sqrt(2) 之外 —— 求盒后必须能罩住。
        let unit = Bounds {
            min: Vec3::splat(-0.5),
            max: Vec3::splat(0.5),
        };
        let rot = Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4);
        let out = unit.transformed(&rot);
        // 转角后 x/z 半宽从 0.5 变成 sqrt(2)/2 ≈ 0.7071
        let expected = 0.5f32 * 2f32.sqrt();
        assert!((out.half_extent().x - expected).abs() < 1e-4, "got {:?}", out.half_extent());
        assert!((out.half_extent().z - expected).abs() < 1e-4);
        // y 不受绕 Y 旋转影响
        assert!((out.half_extent().y - 0.5).abs() < 1e-5);
        assert_eq!(out.center(), Vec3::ZERO);
    }

    #[test]
    fn transformed_empty_bounds_stays_empty() {
        let out = Bounds::EMPTY.transformed(&Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0)));
        assert!(out.is_empty());
    }

    // -- 阴影拟合 --

    #[test]
    fn shadow_fit_of_empty_scene_is_none() {
        assert!(fit_directional_shadow(Vec3::new(0.0, -1.0, 0.0), Bounds::EMPTY).is_none());
    }

    /// 场景包围盒里的任何一点，投影到光源裁剪空间后都必须落在
    /// NDC 的 [-1,1]×[-1,1] 与深度 [0,1] 里。**这是阴影贴图能工作的前提**：
    /// 一旦有点落在外面，那部分是"没有深度可比较"的，画面上就是没有阴影。
    #[test]
    fn shadow_fit_covers_the_whole_scene() {
        let bounds = Bounds {
            min: Vec3::new(-12.0, -1.0, -12.0),
            max: Vec3::new(12.0, 9.0, 12.0),
        };
        for dir in [
            Vec3::new(0.45, -1.0, 0.35),
            Vec3::new(0.0, -1.0, 0.0), // 正上方（会走 up=Z 那条分支）
            Vec3::new(1.0, -0.2, 0.0),
        ] {
            let m = fit_directional_shadow(dir, bounds).expect("should fit");
            for corner in bounds.corners() {
                let ndc = project(&m, corner);
                assert!(
                    ndc.x >= -1.0001 && ndc.x <= 1.0001,
                    "dir {dir:?} corner {corner:?} ndc.x = {}",
                    ndc.x
                );
                assert!(
                    ndc.y >= -1.0001 && ndc.y <= 1.0001,
                    "dir {dir:?} corner {corner:?} ndc.y = {}",
                    ndc.y
                );
                assert!(
                    ndc.z >= -0.0001 && ndc.z <= 1.0001,
                    "dir {dir:?} corner {corner:?} ndc.z = {}",
                    ndc.z
                );
            }
        }
    }

    /// 深度顺序必须与"离光源的远近"一致：更靠近光源的点（逆向光方向）深度更小。
    /// 反了的话比较结果整个反相 —— 该亮的暗、该暗的亮。
    #[test]
    fn shadow_fit_orders_depth_by_distance_to_light() {
        let bounds = Bounds {
            min: Vec3::new(-5.0, -5.0, -5.0),
            max: Vec3::new(5.0, 5.0, 5.0),
        };
        let dir = Vec3::new(0.0, -1.0, 0.0); // 光自上而下
        let m = fit_directional_shadow(dir, bounds).unwrap();

        // 上游（更靠近灯）= +y，下游 = -y。
        let near = project(&m, Vec3::new(0.0, 4.0, 0.0)).z;
        let far = project(&m, Vec3::new(0.0, -4.0, 0.0)).z;
        assert!(near < far, "靠近光源的点深度必须更小：near={near} far={far}");
    }

    #[test]
    fn shadow_fit_handles_zero_direction() {
        let bounds = Bounds {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        };
        // 零向量不该 panic，也不该产生 NaN。
        let m = fit_directional_shadow(Vec3::ZERO, bounds).expect("degenerate dir still fits");
        let ndc = project(&m, Vec3::ZERO);
        assert!(ndc.is_finite(), "零方向拟合出了 NaN：{ndc:?}");
    }

    // -- 阴影光源挑选 --

    #[test]
    fn pick_shadow_slot_chooses_first_shadow_casting_directional() {
        let point = Light::point("lamp", [0.0, 3.0, 0.0], [1.0; 3], 1.0, 10.0);
        let mut sun = Light::directional("sun", [0.5, -1.0, 0.5], [1.0; 3], 1.0);
        sun.casts_shadow = true;
        let mut sun2 = Light::directional("sun2", [0.0, -1.0, 0.0], [1.0; 3], 1.0);
        sun2.casts_shadow = true;

        let packed: Vec<&Light> = vec![&point, &sun, &sun2];
        // 第一节是点光 —— 不支持，跳过；选中的是第二盏（下标 1）。
        assert_eq!(pick_shadow_slot(&packed), Some(1));
    }

    #[test]
    fn pick_shadow_slot_ignores_lights_that_opt_out() {
        let mut off = Light::directional("off", [0.0, -1.0, 0.0], [1.0; 3], 1.0);
        off.casts_shadow = false;
        let packed: Vec<&Light> = vec![&off];
        assert_eq!(pick_shadow_slot(&packed), None);
    }

    /// 打包后的下标必须与 `build_lights` 写进去的位置一致 —— 否则会把
    /// 阴影标记贴到错误的那盏灯上（画面表现为"某个无关的灯在投影"）。
    #[test]
    fn build_lights_marks_the_shadow_slot() {
        let mut sun = Light::directional("sun", [0.0, -1.0, 0.0], [1.0; 3], 1.0);
        sun.casts_shadow = true;
        let lamp = Light::point("lamp", [0.0, 3.0, 0.0], [1.0; 3], 1.0, 10.0);

        let packed: Vec<&Light> = vec![&lamp, &sun];
        let slot = pick_shadow_slot(&packed);
        assert_eq!(slot, Some(1));

        let u = ScenePipeline::build_lights([0.0; 3], &packed, slot);
        assert_eq!(u.ambient[3], 2.0, "两盏灯都应被写入");
        assert_eq!(u.lights[1].shadow_params[0], 1.0, "方向光应被标为投射阴影");
        assert_eq!(u.lights[0].shadow_params[0], 0.0, "点光不该被标");

        // 没有任何灯投射阴影时，标记必须全是 0（片元着色器据此跳过采样）。
        let u_none = ScenePipeline::build_lights([0.0; 3], &packed, None);
        assert_eq!(u_none.lights[0].shadow_params[0], 0.0);
        assert_eq!(u_none.lights[1].shadow_params[0], 0.0);
    }
}
