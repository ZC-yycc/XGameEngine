//! glTF 材质 JSON 的最小投影；纹理与其余 PBR 字段接入时在这里扩展。

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MaterialDef {
    #[serde(default)]
    pub(super) pbr_metallic_roughness: PbrMetallicRoughness,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PbrMetallicRoughness {
    #[serde(default = "white_base_color")]
    pub(super) base_color_factor: [f32; 4],
    /// 基色贴图索引；旧引擎 PBR 的 albedo 贴图。
    #[serde(default)]
    pub(super) base_color_texture: Option<TextureRef>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TextureRef {
    pub(super) index: usize,
}

impl Default for PbrMetallicRoughness {
    fn default() -> Self {
        Self {
            base_color_factor: white_base_color(),
            base_color_texture: None,
        }
    }
}

fn white_base_color() -> [f32; 4] {
    [1.0; 4]
}
