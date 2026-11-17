
struct SkinModel {
    model: mat4x4<f32>,
    color: vec4<f32>,
    // x = metallic，y = roughness
    material: vec4<f32>,
};

@group(3) @binding(0)
var<uniform> skin_model: SkinModel;

@vertex
fn vs(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) joints_idx: vec4<u32>,
    @location(3) weights: vec4<f32>,
) -> SceneVsOut {
    // 变形公式在 skin_vertex.wgsl；这里只补主场景的 model 与共享输出。
    let world = skin_model.model * vec4<f32>(skin_deformed(position, joints_idx, weights), 1.0);
    var out: SceneVsOut;
    out.position = frame.view_proj * world;
    out.world = world.xyz;
    out.color = skin_model.color;
    out.material = skin_model.material;
    out.uv = uv;
    return out;
}
