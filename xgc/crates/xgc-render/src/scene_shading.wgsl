
// 单帧最多参与着色的光源数；与 CPU 侧 `lights::MAX_LIGHTS` 一致。
const MAX_LIGHTS: u32 = 8u;

// 一盏灯：与旧端 `GpuLight` 逐字段对应，也与 `lights.rs` 的打包顺序一致。
struct GpuLight {
    // 方向光：xyz = 光线传播方向（归一化）、w = 0；点光：xyz = 世界坐标、w = 1
    pos_type: vec4<f32>,
    // rgb = 光色，a = 强度
    color_intensity: vec4<f32>,
    // x = 这盏灯是否用方向光阴影图（1 / 0），y = 点光影响半径
    shadow_params: vec4<f32>,
};

struct Frame {
    view_proj: mat4x4<f32>,
    // xyz = 相机世界坐标（用来把面法线朝向相机），w 预留（光源强度在 lights[] 里）
    eye: vec4<f32>,
    ambient: vec4<f32>,
    // 天空漫反射环境（E/π 口径的 L2 球谐），与 sky.rs 的 sh_project 逐项一致
    sky_sh: array<vec4<f32>, 9>,
    // x = 天空是否开启（1 / 0）
    sky_params: vec4<f32>,
    // 4 级级联的光视图投影（级联按视线方向切段）
    light_view_proj: array<mat4x4<f32>, 4>,
    // 每级的远切分距离（世界单位，沿相机视线方向）
    shadow_splits: vec4<f32>,
    // xyz = 相机视线方向（单位向量），片元用它算视线深度选层
    shadow_forward: vec4<f32>,
    // x = 阴影是否开启，y = 深度偏置，w = 级联层数
    shadow_params: vec4<f32>,
    // x = 软阴影（PCSS）的灯光尺寸，y = 级间混合带宽；两个 0 = 与引入画质参数前逐位一致
    shadow_soft: vec4<f32>,
    // x = 有效光源数；y / z / w 预留
    light_count: vec4<f32>,
    lights: array<GpuLight, 8>,
};

@group(0) @binding(0)
var<uniform> frame: Frame;

@group(0) @binding(1)
var shadow_tex: texture_depth_2d_array;

@group(0) @binding(2)
var shadow_smp: sampler_comparison;

@group(2) @binding(0)
var albedo_texture: texture_2d<f32>;

@group(2) @binding(1)
var albedo_sampler: sampler;

// E/π 的球谐求值（系数里已折好余弦卷积）；基函数与 sky.rs 的 sh_basis 逐项一致。
// 与旧引擎同一条下限：粗糙度太低时 GGX 的解析拟合会出 NaN / 过亮高光。
const MIN_ROUGHNESS: f32 = 0.04;

// Lazarov 2013 的 `EnvBRDFApprox`：split-sum 第二项（f0·A + B）的解析拟合。
fn env_brdf_approx(f0: vec3<f32>, roughness: f32, n_dot_v: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * n_dot_v)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

// --- Cook-Torrance 直接光三维项（与旧端 scene_pipeline 逐式一致）-------------
const PI: f32 = 3.141592653589793;

fn distribution_ggx(n_dot_h: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / max(PI * d * d, 1e-6);
}

// Schlick-GGX（直接光的 k = alpha/2 形式）拆成两条可见性函数相乘。
fn smith_visibility(n_dot_v: f32, n_dot_l: f32, alpha: f32) -> f32 {
    let k = alpha * 0.5;
    let gv = n_dot_v / (n_dot_v * (1.0 - k) + k);
    let gl = n_dot_l / (n_dot_l * (1.0 - k) + k);
    return gv * gl;
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - clamp(cos_theta, 0.0, 1.0), 5.0);
}

fn sky_sh_eval(n: vec3<f32>) -> vec3<f32> {
    return frame.sky_sh[0].rgb * 0.282095
         + frame.sky_sh[1].rgb * (0.488603 * n.y)
         + frame.sky_sh[2].rgb * (0.488603 * n.z)
         + frame.sky_sh[3].rgb * (0.488603 * n.x)
         + frame.sky_sh[4].rgb * (1.092548 * n.x * n.y)
         + frame.sky_sh[5].rgb * (1.092548 * n.y * n.z)
         + frame.sky_sh[6].rgb * (0.315392 * (3.0 * n.z * n.z - 1.0))
         + frame.sky_sh[7].rgb * (1.092548 * n.x * n.z)
         + frame.sky_sh[8].rgb * (0.546274 * (n.x * n.x - n.y * n.y));
}

// --- 方向光阴影 ---
//
// 3×3 PCF：把 9 个比较结果平均一下，边缘不那么硬。
//
// 用 `textureSampleCompareLevel` 而不是 `textureSampleCompare`：后者要走隐式导数，
// 而它在片元主路径里，深度图没有 mip，本来也只需要 level 0。
//
// 软阴影（PCSS，`shadow_soft.x > 0`）与级间混合（`shadow_soft.y > 0`）都是**默认关**：
// 关着时走下面这条 3×3 PCF + 硬级间切换的老路径，逐位一致。
//
// 半影估计与软化 PCF 用固定采样盘（poisson）。常量表 ⇒ 跨帧逐位确定 —— 截图
// 确定性是硬要求（无头截图要能逐字节复现），所以这里不用任何随机数。
const BLOCKER_TAPS: i32 = 8;
const BLOCKER_OFFSETS: array<vec2<f32>, 8> = array<vec2<f32>, 8>(
    vec2<f32>(0.0, 0.0),
    vec2<f32>(0.5278, 0.7826),
    vec2<f32>(-0.9231, 0.2345),
    vec2<f32>(0.2341, -0.9214),
    vec2<f32>(-0.4832, -0.7621),
    vec2<f32>(0.8732, -0.3412),
    vec2<f32>(-0.7823, 0.4832),
    vec2<f32>(0.1234, 0.3421),
);
const SOFT_PCF_TAPS: i32 = 12;
const SOFT_PCF_OFFSETS: array<vec2<f32>, 12> = array<vec2<f32>, 12>(
    vec2<f32>(0.0, 0.0),
    vec2<f32>(0.5278, 0.7826),
    vec2<f32>(-0.9231, 0.2345),
    vec2<f32>(0.2341, -0.9214),
    vec2<f32>(-0.4832, -0.7621),
    vec2<f32>(0.8732, -0.3412),
    vec2<f32>(-0.7823, 0.4832),
    vec2<f32>(0.1234, 0.3421),
    vec2<f32>(-0.2341, 0.9214),
    vec2<f32>(0.7621, -0.4832),
    vec2<f32>(-0.9214, -0.2341),
    vec2<f32>(0.3421, -0.8732),
);

// 一层级联里的阴影值。y = 1 表示片元落在这一层的图里（0 = 图外，按"全亮"处理，
// 否则边缘会被裁出一条黑带）。图外 / 关闭时 x 一律 1。
fn cascade_shadow_at(world_pos: vec3<f32>, cascade: u32, ndotl: f32) -> vec2<f32> {
    let light_clip = frame.light_view_proj[cascade] * vec4<f32>(world_pos, 1.0);
    let ndc = light_clip.xyz / light_clip.w;
    // NDC y 朝上、纹理 v 朝下。
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) {
        return vec2<f32>(1.0, 0.0);
    }
    // 掠射角下自遮挡更严重：偏置随 N·L 变小而放大。
    let bias = frame.shadow_params.y * (1.0 + 2.0 * (1.0 - ndotl));
    let depth = ndc.z;
    let texel = 1.0 / f32(textureDimensions(shadow_tex).x);

    if (frame.shadow_soft.x > 0.0) {
        // --- PCSS：先找遮挡物，再按半影宽度做 PCF ---
        //
        // 三步：① 在一个盘里找"比片元更近"的遮挡物，求它们的平均深度；
        // ② 遮挡物离接收面越远，半影越宽（penumbra ∝ receiver - blocker）；
        // ③ 用这个宽度做 PCF。所以贴着墙根的影子硬、离墙远的影子糊 —— 这正是
        // 单一固定核做不到的。
        let search_uv = texel * (2.0 + 8.0 * frame.shadow_soft.x);
        var blocker_sum = 0.0;
        var blocker_count = 0.0;
        for (var i = 0; i < BLOCKER_TAPS; i = i + 1) {
            let coord = uv + BLOCKER_OFFSETS[i] * search_uv;
            if (coord.x < 0.0 || coord.x > 1.0 || coord.y < 0.0 || coord.y > 1.0) {
                continue;
            }
            // `textureLoad` 取**原始深度**（比较采样器只能给 0/1，估不了距离）。
            // 深度数组纹理的 `textureLoad` 要 4 个参数（含 level）：没有 mip，level 恒 0。
            // 下标夹进图内 —— uv 恰好 1.0 时乘出来的下标等于宽度，越界读在 WebGPU 里
            // 返回 0，会被当成"深度 0 的遮挡物"。
            let dims = vec2<i32>(textureDimensions(shadow_tex));
            let pix = clamp(
                vec2<i32>(coord * vec2<f32>(dims)),
                vec2<i32>(i32(0)),
                dims - vec2<i32>(i32(1)),
            );
            let d = textureLoad(shadow_tex, pix, i32(cascade), 0);
            if (d < depth) {
                blocker_sum = blocker_sum + d;
                blocker_count = blocker_count + 1.0;
            }
        }
        if (blocker_count < 0.5) {
            return vec2<f32>(1.0, 1.0);
        }
        let blocker = blocker_sum / blocker_count;
        // 正交光下"遮挡物到接收面的距离"就用深度差近似（光沿 -forward 看，
        // 深度差与沿光距离差只差一个常数倍，收进 light size 里）。
        let penumbra = clamp(
            (depth - blocker) * frame.shadow_soft.x * 12.0,
            texel,
            texel * 24.0,
        );
        var sum = 0.0;
        for (var i = 0; i < SOFT_PCF_TAPS; i = i + 1) {
            let offset = SOFT_PCF_OFFSETS[i] * penumbra;
            sum = sum + textureSampleCompareLevel(
                shadow_tex,
                shadow_smp,
                uv + offset,
                i32(cascade),
                depth - bias,
            );
        }
        return vec2<f32>(sum / f32(SOFT_PCF_TAPS), 1.0);
    }

    // --- 硬阴影：3×3 PCF（与引入画质参数前逐位一致）---
    var sum = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * texel;
            sum = sum + textureSampleCompareLevel(
                shadow_tex,
                shadow_smp,
                uv + offset,
                i32(cascade),
                depth - bias,
            );
        }
    }
    return vec2<f32>(sum / 9.0, 1.0);
}

fn shadow_factor(world_pos: vec3<f32>, ndotl: f32) -> f32 {
    // 选层：级联按**相机视线方向的线性深度**切，splits 是世界单位。
    let count = u32(max(frame.shadow_params.w, 0.0));
    if (count == 0u || frame.shadow_params.x < 0.5) {
        return 1.0;
    }
    let view_depth = dot(world_pos - frame.eye.xyz, frame.shadow_forward.xyz);
    var cascade = count - 1u;
    for (var i = 0u; i + 1u < count; i = i + 1u) {
        if (view_depth <= frame.shadow_splits[i]) {
            cascade = i;
            break;
        }
    }

    let a = cascade_shadow_at(world_pos, cascade, ndotl);
    // --- 级间混合 ---
    //
    // 切分处两级的纹素密度差一截，硬接就是一条看得见的缝。在切分前一条薄带里
    // 把两级混起来（`shadow_soft.y` = 带宽占本级跨度的比例），缝就抹平了。
    //
    // 下一级必须**重叠**着拟合：不重叠的话带里的片元落在下一级的图外，
    // `cascade_shadow_at` 返回"全亮"，混进去等于把缝抹成一圈淡影子。重叠量由
    // CPU 侧 `cascade_shadow_matrices(overlap = cascade_blend)` 给（0 时逐位一致）。
    if (frame.shadow_soft.y > 0.0 && cascade + 1u < count) {
        let split = frame.shadow_splits[cascade];
        let band = split * frame.shadow_soft.y;
        if (view_depth > split - band) {
            let b = cascade_shadow_at(world_pos, cascade + 1u, ndotl);
            if (b.y > 0.5) {
                let t = clamp((view_depth - (split - band)) / max(band, 1e-5), 0.0, 1.0);
                return mix(a.x, b.x, t);
            }
        }
    }
    return a.x;
}

struct SceneVsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) world: vec3<f32>,
    @location(2) @interpolate(flat) material: vec4<f32>,
    @location(3) uv: vec2<f32>,
};

@fragment
fn fs(input: SceneVsOut) -> @location(0) vec4<f32> {
    // 面法线从屏幕导数叉积得到，符号按"朝向相机"归一，闭合网格可见面才受光。
    let raw = cross(dpdx(input.world), dpdy(input.world));
    var normal = normalize(raw + vec3<f32>(0.0, 0.0, 1e-9));
    let to_camera = normalize(frame.eye.xyz - input.world);
    if (dot(normal, to_camera) < 0.0) {
        normal = -normal;
    }
    // 反照率 = 基色因子 × 实体颜色 × 贴图（没贴图时采样 1×1 白纹理）。
    let albedo = input.color.rgb * textureSample(albedo_texture, albedo_sampler, input.uv).rgb;
    let metallic = clamp(input.material.x, 0.0, 1.0);
    let roughness = clamp(input.material.y, MIN_ROUGHNESS, 1.0);
    let n_dot_v = max(dot(normal, to_camera), 1e-4);
    // 绝缘体的基础反射率 4%；金属的 F0 = 自身反照率（有色镜面反射）。
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);
    let alpha_g = roughness * roughness;
    // 环境项①：全局 ambient 当作均匀环境辐射，镜面按菲涅尔加权（旧端同式）。
    let f_env = fresnel_schlick(n_dot_v, f0);
    var light = frame.ambient.rgb * (albedo * (1.0 - metallic) + f_env);
    if (frame.sky_params.x > 0.5) {
        // 环境项②：天空 IBL。漫反射用同一份 L2 球谐；镜面仍是 SH 近似，
        // 但菲涅尔与 kD 折法与旧端 split-sum 一致。
        let f_rough = f0 + (max(vec3<f32>(1.0 - roughness), f0) - f0) * pow(1.0 - n_dot_v, 5.0);
        let kd_env = (vec3<f32>(1.0) - f_rough) * (1.0 - metallic);
        light = light + sky_sh_eval(normal) * albedo * kd_env;
        let reflected = reflect(-to_camera, normal);
        let env_spec = sky_sh_eval(reflected);
        light = light + env_spec * env_brdf_approx(f0, roughness, n_dot_v);
    }
    // 场景光源：方向光 / 点光混排，按盏累加（GGX + Smith + Schlick，漫反射扣 (1-F)）。
    //
    // 只有一盏方向光时这就是老路径：`to_light = normalize(-dir)`、`atten = 1`、
    // radiance 的乘子与阴影采样都和单光源版本逐位一致（证据帧因此逐字节不变）。
    let light_count = u32(max(frame.light_count.x, 0.0));
    for (var i = 0u; i < MAX_LIGHTS; i = i + 1u) {
        if (i >= light_count) {
            break;
        }
        let source = frame.lights[i];
        var to_light: vec3<f32>;
        var atten = 1.0;
        if (source.pos_type.w < 0.5) {
            // 方向光：pos_type.xyz 存的是**传播方向**，表面指向光源要取反。
            to_light = normalize(-source.pos_type.xyz);
        } else {
            // 点光：范围外整盏跳过；范围末段用 smoothstep 收边，别切一刀硬边。
            let to_point = source.pos_type.xyz - input.world;
            let distance = length(to_point);
            let range = source.shadow_params.y;
            if (range <= 0.0 || distance >= range) {
                continue;
            }
            to_light = to_point / max(distance, 1e-4);
            let fade = 1.0 - smoothstep(0.8, 1.0, distance / range);
            atten = fade / (1.0 + 0.09 * distance + 0.032 * distance * distance);
        }
        let n_dot_l = max(dot(normal, to_light), 0.0);
        if (n_dot_l <= 0.0) {
            // 背光面既没有贡献，也没法算半程向量（to_light + to_camera 可能抵消成零向量）。
            continue;
        }
        // 只有打了阴影标记的那盏方向光查 CSM；点光阴影还没做（标记恒 0）。
        var shade = 1.0;
        if (source.shadow_params.x > 0.5) {
            shade = shadow_factor(input.world, n_dot_l);
        }
        // π 校准（与旧端同一条）：场景 intensity 是按 Lambert 直乘手调的，
        // 把 π 并进 radiance，漫反射回到「intensity × 白面 ≈ 1.0」的观感。
        let radiance = source.color_intensity.rgb * source.color_intensity.a * atten * PI;
        let half_dir = normalize(to_light + to_camera);
        let n_dot_h = max(dot(normal, half_dir), 0.0);
        let v_dot_h = max(dot(to_camera, half_dir), 0.0);
        let d_term = distribution_ggx(n_dot_h, alpha_g);
        let g_term = smith_visibility(n_dot_v, n_dot_l, alpha_g);
        let f_term = fresnel_schlick(v_dot_h, f0);
        let specular = d_term * g_term * f_term / max(4.0 * n_dot_v * n_dot_l, 1e-4);
        let kd = (vec3<f32>(1.0) - f_term) * (1.0 - metallic);
        let diffuse = kd * albedo / PI;
        light = light + (diffuse + specular) * radiance * n_dot_l * shade;
    }
    // 保留线性 HDR；整帧只在 tone_map.wgsl 里做一次 ACES。
    return vec4<f32>(light, input.color.a);
}
