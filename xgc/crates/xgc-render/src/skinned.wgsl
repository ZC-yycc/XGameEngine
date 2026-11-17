
struct Joints {
    matrices: array<mat4x4<f32>, 64>,
};

struct SkinModel {
    model: mat4x4<f32>,
    color: vec4<f32>,
    // x = metallic，y = roughness
    material: vec4<f32>,
};

@group(1) @binding(0)
var<uniform> joints: Joints;

@group(3) @binding(0)
var<uniform> skin_model: SkinModel;

@vertex
fn vs(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) joints_idx: vec4<u32>,
    @location(3) weights: vec4<f32>,
) -> SceneVsOut {
    var skinned = vec4<f32>(0.0);
    var weight_sum = 0.0;
    for (var i = 0u; i < 4u; i = i + 1u) {
        if (weights[i] > 0.0) {
            skinned = skinned + (joints.matrices[joints_idx[i]] * vec4<f32>(position, 1.0)) * weights[i];
            weight_sum = weight_sum + weights[i];
        }
    }
    if (weight_sum > 0.0) {
        skinned = skinned / weight_sum;
    } else {
        skinned = vec4<f32>(position, 1.0);
    }
    // CPU 蒙皮只消费变换后的 xyz；把齐次分量固定回 1，避免非仿射输入改变模型平移。
    let world = skin_model.model * vec4<f32>(skinned.xyz, 1.0);
    var out: SceneVsOut;
    out.position = frame.view_proj * world;
    out.world = world.xyz;
    out.color = skin_model.color;
    out.material = skin_model.material;
    out.uv = uv;
    return out;
}
