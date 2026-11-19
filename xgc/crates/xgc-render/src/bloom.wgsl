// Bloom 三级链的唯一着色器实现：亮部提取 / 降采样 / 可分离高斯模糊。
//
// 口径与旧端 `postfx` 的 `BLOOM_WGSL` 相同（阈值落在**曝光后**的线性值上）。
// 曝光来自项目级 `render.exposure`（缺省 1.0），与 tone-map 合成用同一个
// 乘子 —— 只给一处乘曝光，"多亮算亮"就会和实际画面分叉。
// 三个 pass 共用一条 uniform：用不到的字段填中性值，
// 省得维护三套布局与三种 WGSL 结构。

struct BloomParams {
    exposure: f32,
    threshold: f32,
    knee: f32,
    _pad: f32,
    texel: vec2<f32>,      // 源纹理的 texel 尺寸（模糊步长）
    direction: vec2<f32>,  // 模糊方向：(1,0) 横向 / (0,1) 纵向
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_smp: sampler;
@group(0) @binding(2) var<uniform> params: BloomParams;

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
    // 纹理坐标 v 朝下、NDC 的 y 朝上 —— 这里翻一次，光晕才不会上下颠倒。
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

// 亮部提取：曝光 → 按最大通道判亮度 → 软膝曲线 → 原色乘"提取比例"。
//
// 权重是 contribution / brightness（有上界 1）而不是 contribution 本身：
// 软膝区里像素按比例"淡入"光晕，过了膝区后全额参与 —— 光晕不会比源像素还亮
// （Unity URP 的 Bloom Prefilter 就是这个式子）。
//
// 按最大通道判亮度：纯红的高光不该因为"平均亮度低"不发光，彩灯的光晕是彩色的。
@fragment
fn fs_bright(input: VsOut) -> @location(0) vec4<f32> {
    let c = max(textureSample(src_tex, src_smp, input.uv).rgb, vec3<f32>(0.0)) * params.exposure;
    let brightness = max(c.r, max(c.g, c.b));
    // 软膝：贡献从 (threshold - knee) 处的 0 二次升到 (threshold + knee) 处，
    // 之后线性。knee = 0 时退化为硬阈值（分母 +1e-5 兜住除零）。
    var soft = clamp(brightness - params.threshold + params.knee, 0.0, 2.0 * params.knee);
    soft = soft * soft / (4.0 * params.knee + 0.00001);
    let contribution = max(soft, brightness - params.threshold);
    let weight = contribution / max(brightness, 0.00001);
    return vec4<f32>(c * weight, 1.0);
}

// 降采样：输出像素中心恰好落在源 2×2 texel 块的中点（½ 尺寸），一次双线性
// 采样就是一次 2×2 箱式平均 —— 比四次点采样便宜，也没有采样点接缝。
@fragment
fn fs_down(input: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src_tex, src_smp, input.uv);
}

// 可分离二项高斯：权重 [1,8,28,56,70,56,28,8,1]/256（一半点对称写出来）。
// 对 ½ 降采样后的纹理，等效 σ ≈ 2 texel/级；三级叠加得到由近及远的柔和光晕。
@fragment
fn fs_blur(input: VsOut) -> @location(0) vec4<f32> {
    let step = params.texel * params.direction;
    var sum = textureSample(src_tex, src_smp, input.uv).rgb * 0.2734375;        // 70/256
    sum += textureSample(src_tex, src_smp, input.uv + step).rgb * 0.21875;      // 56/256
    sum += textureSample(src_tex, src_smp, input.uv - step).rgb * 0.21875;
    sum += textureSample(src_tex, src_smp, input.uv + step * 2.0).rgb * 0.109375;  // 28/256
    sum += textureSample(src_tex, src_smp, input.uv - step * 2.0).rgb * 0.109375;
    sum += textureSample(src_tex, src_smp, input.uv + step * 3.0).rgb * 0.03125;   // 8/256
    sum += textureSample(src_tex, src_smp, input.uv - step * 3.0).rgb * 0.03125;
    sum += textureSample(src_tex, src_smp, input.uv + step * 4.0).rgb * 0.00390625;  // 1/256
    sum += textureSample(src_tex, src_smp, input.uv - step * 4.0).rgb * 0.00390625;
    return vec4<f32>(sum, 1.0);
}
