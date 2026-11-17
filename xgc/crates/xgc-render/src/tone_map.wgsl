// 全屏合成的唯一 tone-map 真相源；CPU 镜像在 tone.rs。
//
// 一次 pass 做完"光晕合成 + ACES"：光晕必须加在曲线**前** —— 过完曲线的高光
// 没有余量，到显示端再叠加只会被裁出硬边。Bloom 关着时 `intensity == 0`，
// 整段采样被跳过（条件是 uniform，整趟 pass 走同一分支，采样处在一致性
// 控制流里），采样器与三级纹理仍绑着占位资源。

struct BloomParams {
    intensity: f32,  // 0 = 不加光晕（分支跳过，不去采样各级纹理）
    w0: f32,         // 三级光晕的合成权重
    w1: f32,
    w2: f32,
}

@group(0) @binding(0) var hdr_scene: texture_2d<f32>;
@group(0) @binding(1) var glow_sampler: sampler;
@group(0) @binding(2) var<uniform> bloom: BloomParams;
// 三级光晕（½ / ¼ / ⅛）。Bloom 关闭时绑定场景纹理占位 —— 分支不进就不采样。
@group(0) @binding(3) var glow_0: texture_2d<f32>;
@group(0) @binding(4) var glow_1: texture_2d<f32>;
@group(0) @binding(5) var glow_2: texture_2d<f32>;

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

fn aces(linear: vec3<f32>) -> vec3<f32> {
    var c = max(linear, vec3<f32>(0.0)) / 0.6;
    c = aces_input_mat(c);
    let a = c * (c + vec3<f32>(0.0245786)) - vec3<f32>(0.000090537);
    let b = c * (vec3<f32>(0.983729) * c + vec3<f32>(0.432951)) + vec3<f32>(0.238081);
    c = a / b;
    c = aces_output_mat(c);
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

struct VsOut {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs(@builtin(vertex_index) index: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: VsOut;
    out.position = vec4<f32>(corners[index], 0.0, 1.0);
    return out;
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    // position.xy 是像素中心；转整数后与同尺寸 HDR texel 一一对应，不做滤波
    // （场景纹理逐位取原值，光晕才走采样器做双线性放大）。
    let color = textureLoad(hdr_scene, vec2<i32>(input.position.xy), 0);
    var scene = max(color.rgb, vec3<f32>(0.0));
    if (bloom.intensity > 0.0) {
        let uv = input.position.xy / vec2<f32>(textureDimensions(hdr_scene));
        let glow = textureSample(glow_0, glow_sampler, uv).rgb * bloom.w0
            + textureSample(glow_1, glow_sampler, uv).rgb * bloom.w1
            + textureSample(glow_2, glow_sampler, uv).rgb * bloom.w2;
        scene += glow * bloom.intensity;
    }
    // 显示目标是最终不透明帧；材质/粒子/光晕的中间 alpha 不得泄漏到视口。
    return vec4<f32>(aces(scene), 1.0);
}
