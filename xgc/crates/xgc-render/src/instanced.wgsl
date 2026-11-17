
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

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) world: vec3<f32>,
    @location(2) @interpolate(flat) material: vec4<f32>,
};

@vertex
fn vs(@location(0) position: vec3<f32>, @builtin(instance_index) index: u32) -> VsOut {
    let instance = instances[index];
    var out: VsOut;
    let world = instance.model * vec4<f32>(position, 1.0);
    out.position = frame.view_proj * world;
    out.world = world.xyz;
    out.color = instance.color;
    out.material = instance.material;
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
    let to_light = -normalize(frame.sun_dir.xyz);
    let diffuse = max(dot(normal, to_light), 0.0);
    // 环境光 + 天空 IBL（两者同标定，直接相加）+ 场景主方向光。
    let metallic = clamp(input.material.x, 0.0, 1.0);
    let roughness = clamp(input.material.y, MIN_ROUGHNESS, 1.0);
    // 金属没有漫反射：环境与直射的漫反射项都乘 (1 - metallic)。
    let diffuse_color = input.color.rgb * (1.0 - metallic);
    // 介电体 F0 = 0.04，金属 F0 = 基色（旧引擎同样的 mix）。
    let f0 = mix(vec3<f32>(0.04), input.color.rgb, metallic);
    var light = frame.ambient.rgb * diffuse_color;
    if (frame.sky_params.x > 0.5) {
        // 漫反射环境（E/π 的 L2 球谐）+ 环境反射。
        // 反射项与旧引擎 split-sum 同形：`env(R) × (f0·A + B)`；env 用同一份 SH 在
        // 反射方向上求值（预过滤立方体的近似），A/B 用逐材质粗糙度的 Lazarov 拟合。
        light = light + sky_sh_eval(normal) * diffuse_color;
        let n_dot_v = clamp(dot(normal, to_camera), 0.0, 1.0);
        let reflected = reflect(-to_camera, normal);
        let env_spec = sky_sh_eval(reflected);
        light = light + env_spec * env_brdf_approx(f0, roughness, n_dot_v);
    }
    let sun_shadow = shadow_factor(input.world, diffuse);
    light = light + frame.sun_color.rgb * (frame.eye_intensity.w * diffuse * sun_shadow) * diffuse_color;
    let lit = light;
    return vec4<f32>(aces(lit), input.color.a);
}
