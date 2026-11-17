// 蒙皮网格的阴影投射顶点入口：只复用 skin_vertex.wgsl 的变形公式（不参与着色）。
//
// 阴影 pass 的 group(0) 是这一级的光视图投影，因此不能复用主场景的 Frame uniform；
// 这里只声明自己需要的那一个矩阵，画面上的变形与影子的变形仍由同一份 WGSL 计算。

struct ShadowFrame {
    light_view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> shadow_frame: ShadowFrame;

struct ShadowSkinModel {
    model: mat4x4<f32>,
};

@group(2) @binding(0)
var<uniform> shadow_skin_model: ShadowSkinModel;

@vertex
fn vs(
    @location(0) position: vec3<f32>,
    @location(2) joints_idx: vec4<u32>,
    @location(3) weights: vec4<f32>,
) -> @builtin(position) vec4<f32> {
    let world = shadow_skin_model.model * vec4<f32>(skin_deformed(position, joints_idx, weights), 1.0);
    return shadow_frame.light_view_proj * world;
}
