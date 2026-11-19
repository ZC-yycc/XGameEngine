
struct Instance {
    model: mat4x4<f32>,
    color: vec4<f32>,
    // x = metallic，y = roughness
    material: vec4<f32>,
};

@group(1) @binding(0)
var<storage, read> instances: array<Instance>;

@vertex
fn vs(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    // 模型空间法线；全零 = 模型没导法线（片元退回屏幕导数）。
    @location(2) normal: vec3<f32>,
    @builtin(instance_index) index: u32,
) -> SceneVsOut {
    let instance = instances[index];
    var out: SceneVsOut;
    let world = instance.model * vec4<f32>(position, 1.0);
    out.position = frame.view_proj * world;
    out.world = world.xyz;
    out.color = instance.color;
    out.material = instance.material;
    out.uv = uv;
    out.normal = normal_matrix(instance.model) * normal;
    return out;
}
