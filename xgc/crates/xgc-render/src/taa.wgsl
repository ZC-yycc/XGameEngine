//! 时间抗锯齿的历史解析：当前帧颜色 + 深度反投影到上一帧 + 邻域钳制的历史混合。
//!
//! 与旧端 `xgame-render/src/taa.rs` 的 `TAA_WGSL` 同口径：反投影用上一帧**未抖动**的
//! 视图投影，再补上上一帧的抖动偏移；深度 1 附近（天空）按视线方向推远，避免把天空
//! 误判成静止几何；历史先被当前帧 3×3 邻域钳制再混合，抑制拖影。`blend = 0` 即直通
//! 当前帧（首帧、重投影越界、相机背后），保证默认关 / 重置后的第一帧与不开 TAA 逐位
//! 一致。

struct TaaParams {
    inv_view_proj: mat4x4<f32>,
    prev_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    prev_jitter_uv: vec2<f32>,
    texel: vec2<f32>,
    blend: f32,
    clamp_scale: f32,
    resolution: vec2<f32>,
};

@group(0) @binding(0) var current_tex: texture_2d<f32>;
@group(0) @binding(1) var depth_tex: texture_depth_2d;
@group(0) @binding(2) var history_tex: texture_2d<f32>;
@group(0) @binding(3) var linear_sampler: sampler;
@group(0) @binding(4) var<uniform> params: TaaParams;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) clip: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = clip.xy / params.resolution;
    let current = textureSampleLevel(current_tex, linear_sampler, uv, 0.0);
    // 首帧 / 刚重建历史：没有可信的上一帧，直接给当前帧。
    if params.blend <= 0.0 {
        return current;
    }
    let pixel = vec2<i32>(clip.xy);
    let max_pixel = vec2<i32>(params.resolution) - vec2<i32>(1, 1);
    let d = textureLoad(depth_tex, clamp(pixel, vec2<i32>(0, 0), max_pixel), 0);
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let h = params.inv_view_proj * vec4<f32>(ndc, d, 1.0);
    if abs(h.w) < 1.0e-6 {
        return current;
    }
    var world = h.xyz / h.w;
    // 天空深度退到 1：按方向推到很远处，只保留旋转视差（与旧端同式）。
    if d >= 0.99999 {
        let direction = normalize(world - params.camera_pos.xyz);
        world = params.camera_pos.xyz + direction * 1.0e4;
    }
    let previous = params.prev_view_proj * vec4<f32>(world, 1.0);
    if previous.w <= 1.0e-6 {
        return current;
    }
    let previous_ndc = previous.xy / previous.w;
    let previous_uv = vec2<f32>(
        previous_ndc.x * 0.5 + 0.5,
        0.5 - previous_ndc.y * 0.5,
    ) + params.prev_jitter_uv;
    if previous_uv.x < 0.0 || previous_uv.x > 1.0 || previous_uv.y < 0.0 || previous_uv.y > 1.0 {
        return current;
    }
    var lo = current;
    var hi = current;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let coord = clamp(pixel + vec2<i32>(x, y), vec2<i32>(0, 0), max_pixel);
            let neighbour = textureLoad(current_tex, coord, 0);
            lo = min(lo, neighbour);
            hi = max(hi, neighbour);
        }
    }
    let spread = (hi - lo) * params.clamp_scale;
    var history = textureSampleLevel(history_tex, linear_sampler, previous_uv, 0.0);
    history = clamp(history, lo - spread, hi + spread);
    return mix(current, history, params.blend);
}
