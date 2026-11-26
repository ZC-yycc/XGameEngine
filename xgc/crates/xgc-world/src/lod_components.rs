//! LOD 层级按从近到远的顺序保存，模型和距离可分别编辑。

use xgc_reflect::{Component, ReflectValue};

#[derive(Clone, Debug, PartialEq, Default, ReflectValue)]
#[reflect(element_name = "level")]
pub struct LodLevel {
    #[field(asset = Model, doc = "内建图元、模型 GUID 或项目相对路径")]
    pub model: String,
    #[field(unit = "m", range = (-(f32::MAX as f64), f32::MAX as f64), doc = "此级的最大距离，零或负值表示无限距离")]
    pub max_distance: f32,
}

#[derive(Clone, Debug, PartialEq, Default, Component)]
#[component(
    id = 0x0204,
    name = "render.lod",
    doc = "由近到远的 LOD 模型与距离配置"
)]
pub struct LodGroup {
    #[field(doc = "按从近到远排列，空列表表示未配置 LOD")]
    pub levels: Vec<LodLevel>,
}
