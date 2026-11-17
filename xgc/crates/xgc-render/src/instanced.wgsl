
struct Frame {
    view_proj: mat4x4<f32>,
    // xyz = 相机世界坐标（用来把面法线朝向相机），w = 方向光强度
    eye_intensity: vec4<f32>,
    ambient: vec4<f32>,
    // xyz = 光线传播方向（归一化）
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    // 天空漫反射环境（E/π 口径的 L2 球谐），与 sky.rs 的 sh_project 逐项一致
    sky_sh: array<vec4<f32>, 9>,
    // x = 天空是否开启（1 / 0）
    sky_params: vec4<f32>,
    light_view_proj: mat4x4<f32>,
    // x = 阴影是否开启，y = 1/图宽，z = 深度偏置
    shadow_params: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> frame: Frame;

// 色调映射：与 tone.rs 的 Rust 镜像逐式对应（ACES RRT+ODT 拟合，1/0.6 亮度基准确认）。
// 旧引擎在合成 pass 里过这条曲线；这里在片元输出前套同一条，随后由 sRGB 目标编码。
fn aces_input_mat(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        0.59719 * c.x + 0.35458 * c.y + 0.04823 * c.z,
        0.07600 * c.x + 0.90834 * c.y + 0.01566 * c.z,
        0.02840 * c.x + 0.13383 * c.y + 0.83777 * c.z,
    );
}

fn aces_output_mat(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        1.60475 * c.x - 0.53108 * c.y - 0.07367 * c.z,
        -0.10208 * c.x + 1.10813 * c.y - 0.00605 * c.z,
        -0.00327 * c.x - 0.07276 * c.y + 1.07602 * c.z,
    );
}

fn aces_rrt(c: vec3<f32>) -> vec3<f32> {
    let a = c * (c + vec3<f32>(0.0245786)) - vec3<f32>(0.000090537);
    let b = c * (vec3<f32>(0.983729) * c + vec3<f32>(0.432951)) + vec3<f32>(0.238081);
    return a / b;
}

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

// 3×3 PCF（与旧引擎"硬阴影"分支同式）：图外 / 关闭时一律返回 1（全亮）。
fn shadow_factor(world_pos: vec3<f32>, ndotl: f32) -> f32 {
    if (frame.shadow_params.x < 0.5) {
        return 1.0;
    }
    let clip = frame.light_view_proj * vec4<f32>(world_pos, 1.0);
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0;
    }
    // 掠射角下采样跨度大、自遮挡更重，偏置随 N·L 变小而放大。
    let bias = frame.shadow_params.z * (1.0 + 2.0 * (1.0 - ndotl));
    let depth = ndc.z;
    var sum = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * frame.shadow_params.y;
            sum = sum + textureSampleCompareLevel(shadow_tex, shadow_smp, uv + offset, depth - bias);
        }
    }
    return sum / 9.0;
}

fn aces(linear: vec3<f32>) -> vec3<f32> {
    var c = max(linear, vec3<f32>(0.0)) / 0.6;
    c = aces_input_mat(c);
    c = aces_rrt(c);
    c = aces_output_mat(c);
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

struct Instance {
    model: mat4x4<f32>,
    color: vec4<f32>,
    // x = metallic，y = roughness
    material: vec4<f32>,
};

@group(0) @binding(1)
var shadow_tex: texture_depth_2d;

@group(0) @binding(2)
var shadow_smp: sampler_comparison;

@group(1) @binding(0)
var<storage, read> instances: array<Instance>;

@group(2) @binding(0)
var albedo_texture: texture_2d<f32>;

@group(2) @binding(1)
var albedo_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) world: vec3<f32>,
    @location(2) @interpolate(flat) material: vec4<f32>,
    @location(3) uv: vec2<f32>,
};

@vertex
fn vs(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @builtin(instance_index) index: u32,
) -> VsOut {
    let instance = instances[index];
    var out: VsOut;
    let world = instance.model * vec4<f32>(position, 1.0);
    out.position = frame.view_proj * world;
    out.world = world.xyz;
    out.color = instance.color;
    out.material = instance.material;
    out.uv = uv;
    return out;
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    // 面法线从屏幕导数叉积得到，符号按"朝向相机"归一，闭合网格可见面才受光。
    let raw = cross(dpdx(input.world), dpdy(input.world));
    var normal = normalize(raw + vec3<f32>(0.0, 0.0, 1e-9));
    let to_camera = normalize(frame.eye_intensity.xyz - input.world);
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
    // 场景主方向光：GGX 分布 + Smith 可见性 + Schlick 菲涅尔，漫反射按 (1-F) 扣减。
    let to_light = -normalize(frame.sun_dir.xyz);
    let n_dot_l = max(dot(normal, to_light), 0.0);
    let sun_shadow = shadow_factor(input.world, n_dot_l);
    // π 校准（与旧端同一条）：场景 intensity 是按 Lambert 直乘手调的，
    // 把 π 并进 radiance，漫反射回到「intensity × 白面 ≈ 1.0」的观感。
    let radiance = frame.sun_color.rgb * frame.eye_intensity.w * PI;
    let half_dir = normalize(to_light + to_camera);
    let n_dot_h = max(dot(normal, half_dir), 0.0);
    let v_dot_h = max(dot(to_camera, half_dir), 0.0);
    let d_term = distribution_ggx(n_dot_h, alpha_g);
    let g_term = smith_visibility(n_dot_v, n_dot_l, alpha_g);
    let f_term = fresnel_schlick(v_dot_h, f0);
    let specular = d_term * g_term * f_term / max(4.0 * n_dot_v * n_dot_l, 1e-4);
    let kd = (vec3<f32>(1.0) - f_term) * (1.0 - metallic);
    let diffuse = kd * albedo / PI;
    light = light + (diffuse + specular) * radiance * n_dot_l * sun_shadow;
    let lit = light;
    return vec4<f32>(aces(lit), input.color.a);
}
