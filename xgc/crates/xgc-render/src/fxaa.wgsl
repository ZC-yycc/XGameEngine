// FXAA 3.11 的质量路径（5 抽头 + 沿边搜索）。
//
// 逐行对齐旧端 `xgame-render/src/postfx.rs` 的 `FXAA_WGSL`：亚像素混合 0.75、
// 方向收缩 1/8（下界 1/128）、跨度上限 8 texel 都是 FXAA 3.11 的公开口径；
// 边缘阈值取 1/12 与 1/6（stock 的 1/24 与 1/8 更松，会把整幅画面柔化）。
//
// 采样一律 `textureSampleLevel(..., 0.0)`：后处理是 1:1，LOD 本就是 0，而边缘
// 检测的分支建立在采样值上，WGSL 的一致性控制流规则不允许隐式 LOD 采样。

struct FxaaParams {
    // 源纹理的 texel 尺寸（边缘搜索步长的基数）。
    texel: vec2<f32>,
    // 1 = 源 / 目标是 sRGB 格式：采样值是线性的，需先编码到显示空间。
    srgb: u32,
    _pad: u32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_smp: sampler;
@group(0) @binding(2) var<uniform> params: FxaaParams;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs(@builtin(vertex_index) index: u32) -> VsOut {
    var xy = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let p = xy[index];
    var out: VsOut;
    out.clip = vec4<f32>(p, 0.0, 1.0);
    // 与 tone-map 同一套 uv 约定：clip 的 +Y 朝上，纹理的 +V 朝下。
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

fn luma(c: vec3<f32>) -> f32 {
    // Rec.601 权重：FXAA 原实现的 luma 口径，与阈值常量配套，别换成 Rec.709。
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(lo, hi, c > vec3<f32>(0.0031308));
}

fn srgb_decode(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(lo, hi, c > vec3<f32>(0.04045));
}

// sRGB 目标的采样值是硬件解码后的线性光；luma 阈值是显示空间的量，先编码再比较。
// 非 sRGB 目标存的本来就是显示值（tone-map 写入时没有硬件编码）。
fn to_display(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    if (params.srgb == 1u) {
        return srgb_encode(x);
    }
    return x;
}

// 混合结果（显示空间）→ 目标着色器输出空间。sRGB 目标要解码回线性，由硬件在
// 写入时再编码一次；非 sRGB 目标原样写。
fn from_display(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    if (params.srgb == 1u) {
        return srgb_decode(x);
    }
    return x;
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    let t = params.texel;
    let uv = input.uv;
    let raw_m = textureSampleLevel(src_tex, src_smp, uv, 0.0).rgb;
    let c_m = to_display(raw_m);
    let c_nw = to_display(textureSampleLevel(src_tex, src_smp, uv + vec2<f32>(-1.0, -1.0) * t, 0.0).rgb);
    let c_ne = to_display(textureSampleLevel(src_tex, src_smp, uv + vec2<f32>( 1.0, -1.0) * t, 0.0).rgb);
    let c_sw = to_display(textureSampleLevel(src_tex, src_smp, uv + vec2<f32>(-1.0,  1.0) * t, 0.0).rgb);
    let c_se = to_display(textureSampleLevel(src_tex, src_smp, uv + vec2<f32>( 1.0,  1.0) * t, 0.0).rgb);

    let l_m = luma(c_m);
    let l_nw = luma(c_nw);
    let l_ne = luma(c_ne);
    let l_sw = luma(c_sw);
    let l_se = luma(c_se);
    let l_min = min(l_m, min(min(l_nw, l_ne), min(l_sw, l_se)));
    let l_max = max(l_m, max(max(l_nw, l_ne), max(l_sw, l_se)));
    let contrast = l_max - l_min;

    // 亚像素项：中心与四邻均值的差占局部对比度的比例，平滑后按 0.75 混入。
    // 平坦区域这一项为 0 —— 这是"不做整幅柔焦"的第一道闸门。
    let average = (c_nw + c_ne + c_sw + c_se) * 0.25;
    let ratio = clamp(abs(luma(average) - l_m) / max(contrast, 0.0001), 0.0, 1.0);
    let subpix = smoothstep(0.0, 1.0, ratio) * 0.75;

    // 五抽头范围低于阈值 ⇒ 无边缘。原样返回中心像素（不走 encode/decode 往返，
    // 保证平坦区逐字节不变）；亚像素项非零时才用混合结果。
    if (contrast < max(1.0 / 12.0, l_max * (1.0 / 6.0))) {
        if (subpix <= 0.0) {
            return vec4<f32>(raw_m, 1.0);
        }
        return vec4<f32>(from_display(mix(c_m, average, subpix)), 1.0);
    }

    // 边缘方向：对角亮度差之和的梯度方向。dirReduce 防止平坦方向上的除零 /
    // 爆炸；再按 8 texel 上限收缩（步子太大会把远处内容拖进来）。
    var dir = vec2<f32>(
        -((l_nw + l_ne) - (l_sw + l_se)),
        ((l_nw + l_sw) - (l_ne + l_se)),
    );
    let dir_reduce = max((l_nw + l_ne + l_sw + l_se) * 0.25 * 0.125, 1.0 / 128.0);
    let rcp_dir_min = 1.0 / (min(abs(dir.x), abs(dir.y)) + dir_reduce);
    dir = clamp(dir * rcp_dir_min, vec2<f32>(-8.0), vec2<f32>(8.0)) * t;

    // 沿边缘方向取 4 个点（1/3、2/3 处的两点先平均成 A；±½ 处两点与 A 再平均成 B）。
    // B 超出局部亮度范围说明混进了别的结构，退回 A。
    let c_a = 0.5 * (
        to_display(textureSampleLevel(src_tex, src_smp, uv + dir * (1.0 / 3.0 - 0.5), 0.0).rgb)
        + to_display(textureSampleLevel(src_tex, src_smp, uv + dir * (2.0 / 3.0 - 0.5), 0.0).rgb)
    );
    let c_b = c_a * 0.5 + 0.25 * (
        to_display(textureSampleLevel(src_tex, src_smp, uv - dir * 0.5, 0.0).rgb)
        + to_display(textureSampleLevel(src_tex, src_smp, uv + dir * 0.5, 0.0).rgb)
    );
    let l_b = luma(c_b);
    let c_out = select(c_b, c_a, l_b < l_min || l_b > l_max);
    return vec4<f32>(from_display(c_out), 1.0);
}
