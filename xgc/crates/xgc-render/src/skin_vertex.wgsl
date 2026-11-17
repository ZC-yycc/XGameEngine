// GPU 蒙皮顶点公式的唯一真相源：主场景入口（skinned.wgsl）与阴影投射入口
// （shadow_skin.wgsl）都 include 这一份，避免"画面上是一套骨骼、影子里是另一套"。
//
// 这里只放 palette 绑定与变形函数：光 / 相机矩阵由各自的入口声明，因为主场景的
// group(0) 是 Frame、阴影 pass 的 group(0) 是光视图投影，两者不能共用。

// 与 skinned.rs 的 MAX_JOINTS 同值：256 槽对齐旧引擎 BONES_PER_SLOT。
const MAX_JOINTS: u32 = 256u;

struct SkinPalette {
    matrices: array<mat4x4<f32>, MAX_JOINTS>,
};

@group(1) @binding(0)
var<uniform> skin_palette: SkinPalette;

// 顶点矩阵 = Σ wₖ · palette[jointsₖ] · position，再按 Σwₖ 归一。
// 全零权重（或越界下标但没有权重）的顶点保持原位：与 export 时"未知骨冻结"同一口径。
fn skin_deformed(position: vec3<f32>, joint_index: vec4<u32>, weights: vec4<f32>) -> vec3<f32> {
    var skinned = vec4<f32>(0.0);
    var weight_sum = 0.0;
    for (var i = 0u; i < 4u; i = i + 1u) {
        if (weights[i] > 0.0) {
            skinned = skinned
                + (skin_palette.matrices[joint_index[i]] * vec4<f32>(position, 1.0)) * weights[i];
            weight_sum = weight_sum + weights[i];
        }
    }
    if (weight_sum > 0.0) {
        // CPU 蒙皮只消费变换后的 xyz；把齐次分量固定回 1，避免非仿射输入改变模型平移。
        return (skinned / weight_sum).xyz;
    }
    return position;
}
