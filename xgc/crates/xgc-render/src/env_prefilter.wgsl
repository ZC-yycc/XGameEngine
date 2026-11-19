// 环境立方体的 GPU 预过滤：解析天空 → 5 级 GGX 预过滤立方体 + 一张 BRDF 积分表。
//
// 为什么不用解析球谐近似：镜面反射是**方向性**的，L2 球谐（9 个系数）把高光抹成
// 一大片钝光，彩色介质面上会浮出偏蓝的假高光 —— 旧端用预过滤立方体 + BRDF LUT
// 的 split-sum 正是为了避免这一点，这里逐行移植同一套数学。
//
// 与旧端 `sky.rs` 一致的两条纪律：
// 1. 采样方向全部来自固定 Hammersley 序列（无随机数），所以同一帧反复渲染逐字节
//    相同、帧 150 截图可复现；
// 2. 采样数按级数由 CPU 通过 uniform 传入（96 / 48 / 24 / 16 / 12），不是编译期常量。

// 每 (level, face) 一份：面基向量 + 该级的粗糙度与采样数。
// 与 `env_map.rs` 的 `PrefilterParams` 逐字节对应（48B）。
struct PrefilterParams {
    // xyz = 面的朝向（未归一，组合后归一）
    forward: vec4<f32>,
    // xyz = 面的右轴，w = 该级的粗糙度（level / (ENV_MIPS - 1)）
    right: vec4<f32>,
    // xyz = 面的上轴，w = 该级的采样数
    up: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> sky_uniform: SkyUniform;

@group(0) @binding(1)
var<uniform> params: PrefilterParams;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// 全屏三角形：与旧端同式（uv 的 y 翻过，所以插值出的 uv 直接是纹理坐标）。
@vertex
fn vs(@builtin(vertex_index) index: u32) -> VsOut {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VsOut;
    out.position = vec4<f32>(vec2<f32>(uv.x, 1.0 - uv.y) * 2.0 - 1.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}

// Van der Corput 反演（位翻转）。
fn radical_inverse_vdc(bits_in: u32) -> f32 {
    var bits = bits_in;
    bits = (bits << 16u) | (bits >> 16u);
    bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
    bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
    bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
    bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
    return f32(bits) * 2.3283064365386963e-10;
}

fn hammersley(index: u32, count: u32) -> vec2<f32> {
    return vec2<f32>(f32(index) / f32(count), radical_inverse_vdc(index));
}

// GGX 重要性采样：按粗糙度把半程向量聚到法线附近。
fn importance_sample_ggx(xi: vec2<f32>, n: vec3<f32>, roughness: f32) -> vec3<f32> {
    let a = roughness * roughness;
    let phi = 2.0 * SKY_PI * xi.x;
    let cos_theta = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
    let sin_theta = sqrt(max(1.0 - cos_theta * cos_theta, 0.0));
    let h = vec3<f32>(sin_theta * cos(phi), sin_theta * sin(phi), cos_theta);
    // 法线接近 ±Z 时换一条切线轴，避免叉积退化。
    let up = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.z) > 0.999);
    let tangent = normalize(cross(up, n));
    let bitangent = cross(n, tangent);
    return normalize(tangent * h.x + bitangent * h.y + n * h.z);
}

// Karis 的 Schlick-GGX 可见性项（BRDF 积分表用，k = r²/2）。
// 注意这里**不是**直接光的 k = (r+1)²/8：那是带分母 (N·L)(N·V) 的解析式，
// 而 LUT 积的是已经除掉分母的 G_vis，必须用 IBL 的 k = r²/2，否则掠射角下
// 平滑金属的 A 会被压低一半以上（实测 0.04 粗糙度、N·V=0.2 时 0.668 → 0.283）。
fn geometry_schlick_ggx(n_dot_x: f32, roughness: f32) -> f32 {
    let k = (roughness * roughness) / 2.0;
    return n_dot_x / (n_dot_x * (1.0 - k) + k);
}

// 一级一面的 GGX 预过滤：V = N = R（split-sum 的约定），只累积辐亮度。
@fragment
fn fs_prefilter(input: VsOut) -> @location(0) vec4<f32> {
    // 面的基向量不是归一化的：屏幕 uv 在 [0,1] 上展开成一条射线，归一后才是方向。
    let dir = normalize(
        params.forward.xyz
            + params.right.xyz * (2.0 * input.uv.x - 1.0)
            + params.up.xyz * (1.0 - 2.0 * input.uv.y),
    );
    let roughness = params.right.w;
    let samples = u32(params.up.w);
    let n = dir;
    let v = dir;
    var accumulated = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i = 0u; i < samples; i = i + 1u) {
        let h = importance_sample_ggx(hammersley(i, samples), n, roughness);
        let l = normalize(2.0 * dot(v, h) * h - v);
        let n_dot_l = max(dot(n, l), 0.0);
        if (n_dot_l > 0.0) {
            accumulated = accumulated + sky_radiance(sky_uniform, l) * n_dot_l;
            weight = weight + n_dot_l;
        }
    }
    return vec4<f32>(accumulated / max(weight, 1e-5), 1.0);
}

// BRDF LUT：x = N·V、y = roughness；输出 (A, B)，着色端按 f0·A + B 组合。
// 256 个固定样本的 Karis 积分，与旧端逐式一致（所以 LUT 不需要每一帧重建）。
@fragment
fn fs_brdf(input: VsOut) -> @location(0) vec4<f32> {
    let n_dot_v = clamp(input.uv.x, 0.001, 1.0);
    let roughness = clamp(input.uv.y, 0.001, 1.0);
    // N·V 夹到 1 时 1-(N·V)² 可能因舍入变负，旧端同样夹 0 再开根。
    let v = vec3<f32>(sqrt(max(1.0 - n_dot_v * n_dot_v, 0.0)), 0.0, n_dot_v);
    let n = vec3<f32>(0.0, 0.0, 1.0);
    var a = 0.0;
    var b = 0.0;
    let samples = 256u;
    for (var i = 0u; i < samples; i = i + 1u) {
        let h = importance_sample_ggx(hammersley(i, samples), n, roughness);
        let l = normalize(2.0 * dot(v, h) * h - v);
        let n_dot_l = max(l.z, 0.0);
        if (n_dot_l > 0.0) {
            let n_dot_h = max(h.z, 0.0);
            let v_dot_h = max(dot(v, h), 0.0);
            let g = geometry_schlick_ggx(n_dot_v, roughness)
                * geometry_schlick_ggx(n_dot_l, roughness);
            let g_vis = g * v_dot_h / (n_dot_h * n_dot_v + 1e-6);
            let fc = pow(1.0 - v_dot_h, 5.0);
            a = a + (1.0 - fc) * g_vis;
            b = b + fc * g_vis;
        }
    }
    return vec4<f32>(a / f32(samples), b / f32(samples), 0.0, 1.0);
}
