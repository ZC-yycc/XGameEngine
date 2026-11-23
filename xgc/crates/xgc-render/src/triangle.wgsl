struct Material {
    color: vec4<f32>,
    mvp: mat4x4<f32>,
    rect: vec4<f32>,
    border_color: vec4<f32>,
    radius: f32,
    border_width: f32,
};

@group(0) @binding(0)
var<uniform> material: Material;

@vertex
fn vs(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return material.mvp * vec4<f32>(position, 1.0);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // 与旧端 `ui_renderer` 的圆角矩形同一套 SDF：1px 过渡带做抗锯齿（UI 是像素
    // 对齐的，宽带只会糊）；`border_width > 0` 时把内缩后的环带混成边框色。
    // 圆角与边框都为 0 时直接返回实例色：全屏三角形 / 清屏这些路径行为不变。
    if (material.radius > 0.0 || material.border_width > 0.0) {
        let half = material.rect.zw * 0.5;
        let local = position.xy - (material.rect.xy + half);
        let r = min(material.radius, min(half.x, half.y));
        let q = abs(local) - (half - vec2<f32>(r, r));
        let distance = length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
        let fill = 1.0 - smoothstep(-0.5, 0.5, distance);
        if (material.border_width > 0.0) {
            let inner = 1.0 - smoothstep(-0.5, 0.5, distance + material.border_width);
            let border = max(fill - inner, 0.0);
            let weight = border / max(fill, 1e-5);
            let rgb = mix(material.color.rgb, material.border_color.rgb, weight);
            let alpha = material.color.a * inner + material.border_color.a * border;
            return vec4<f32>(rgb, alpha);
        }
        return vec4<f32>(material.color.rgb, material.color.a * fill);
    }
    return material.color;
}
