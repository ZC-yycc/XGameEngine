// 全屏合成的唯一 tone-map 真相源；CPU 镜像在 tone.rs。
@group(0) @binding(0)
var hdr_scene: texture_2d<f32>;

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
    // position.xy 是像素中心；转整数后与同尺寸 HDR texel 一一对应，不做滤波。
    let color = textureLoad(hdr_scene, vec2<i32>(input.position.xy), 0);
    // 显示目标是最终不透明帧；材质/粒子的中间 alpha 不得泄漏到视口。
    return vec4<f32>(aces(color.rgb), 1.0);
}
