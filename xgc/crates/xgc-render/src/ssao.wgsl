// SSAO 的三个片元入口（AO 生成 / 可分离模糊 / 乘回场景色），一份模块给三支管线用。
// 与旧端 `ssao.rs` 内联 WGSL 逐行同式：半分辨率深度遮蔽、深度梯度重建法线、
// 像素整数哈希旋转半球 kernel（确定性噪声），AO 图先模糊再线性上采样乘算。

struct SsaoParams {
    inv_view_proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    kernel: array<vec4<f32>, 16>,
    // x = 采样半径（世界单位）、y = 深度偏置、z = 强度（0 = 无遮蔽）。
    params: vec4<f32>,
    // 全分辨率：xy = 像素数、zw = 1 / 像素数。
    res: vec4<f32>,
    // 半分辨率：xy = 像素数、zw = 1 / 像素数（模糊的边界夹取用）。
    half_res: vec4<f32>,
};

// 一支管线只会用到其中一部分绑定：AO 读深度与 uniform，模糊读半分辨率 AO，
// 乘算读场景色 + AO。BGL 只声明各自用到的那几项。
@group(0) @binding(0) var depth_tex: texture_depth_2d;
@group(0) @binding(1) var ao_in: texture_2d<f32>;
@group(0) @binding(2) var color_in: texture_2d<f32>;
@group(0) @binding(3) var color_sampler: sampler;
@group(0) @binding(4) var<uniform> u: SsaoParams;

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
    // 纹理坐标 v 朝下、NDC 的 y 朝上 —— 翻一次（与 TAA / 光晕同一约定）。
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

fn world_from_depth(pixel: vec2<i32>) -> vec3<f32> {
    let uv = (vec2<f32>(pixel) + vec2<f32>(0.5, 0.5)) * u.res.zw;
    let d = textureLoad(depth_tex, pixel, 0);
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let h = u.inv_view_proj * vec4<f32>(ndc, d, 1.0);
    return h.xyz / h.w;
}

/// 像素整数哈希 → [0,1) 的旋转角比例：纯整数运算，跨运行逐位一致（截图可复现）。
fn hash_angle(p: vec2<i32>) -> f32 {
    var h = u32(p.x) * 1973u + u32(p.y) * 9277u + 1u;
    h = (h ^ (h >> 13u)) * 1274126177u;
    return f32(h & 0xFFFFu) / 65536.0;
}

@fragment
fn fs_ao(input: VsOut) -> @location(0) vec4<f32> {
    // 强度 0 = 无遮蔽：矩阵退化时 CPU 也把强度写成 0（反投影出垃圾坐标时
    // 遮蔽只会是噪声，直通才是对的）。
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
        // 场景面比采样点更近（超出偏置）→ 采样点被挡；遮挡物与中心点
        // 深度差太远时视为"别的物体"不算遮蔽（Crytek 式 range check）。
        if (sample_dist - scene_dist > bias) {
            let range = smoothstep(0.0, 1.0, radius / max(abs(scene_dist - center_dist), 1.0e-4));
            occluded = occluded + range;
        }
    }
    let ao = 1.0 - clamp(occluded / 16.0 * strength, 0.0, 1.0);
    return vec4<f32>(ao, ao, ao, 1.0);
}

/// 可分离 5 抽头 [1,4,6,4,1]/16 模糊（半分辨率，横竖各一趟）。
fn blur(input: VsOut, dir: vec2<i32>) -> vec4<f32> {
    let px = vec2<i32>(input.clip.xy);
    let max_px = vec2<i32>(u.half_res.xy) - vec2<i32>(1, 1);
    var weights = array<f32, 5>(1.0, 4.0, 6.0, 4.0, 1.0);
    var sum = 0.0;
    for (var i = -2; i <= 2; i = i + 1) {
        let q = clamp(px + dir * i, vec2<i32>(0), max_px);
        sum = sum + textureLoad(ao_in, q, 0).r * weights[i + 2];
    }
    return vec4<f32>(sum / 16.0, 0.0, 0.0, 1.0);
}

@fragment
fn fs_blur_h(input: VsOut) -> @location(0) vec4<f32> {
    return blur(input, vec2<i32>(1, 0));
}

@fragment
fn fs_blur_v(input: VsOut) -> @location(0) vec4<f32> {
    return blur(input, vec2<i32>(0, 1));
}

@fragment
fn fs_apply(input: VsOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(input.clip.xy);
    let color = textureLoad(color_in, px, 0);
    // 半分辨率 AO 线性上采样到全分辨率（顺带把 2×2 块的缝抹掉）。
    let ao = textureSampleLevel(ao_in, color_sampler, input.uv, 0.0).r;
    return vec4<f32>(color.rgb * ao, color.a);
}
