//! LOD 层级按从近到远的顺序保存，模型和距离可分别编辑。

use xgc_base::EntityKey;
use xgc_reflect::{Component, ReflectValue};

use crate::{MeshRenderer, World};

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

impl World {
    /// 取景、拾取与根运动参考同一首个可绘制模型。
    pub fn primary_model(&self, key: EntityKey) -> Option<&str> {
        self.model_at_distance(key, || 0.0, 0.0)
    }

    /// 按世界轴心距离选择模型。只有配置且启用 LOD 时才计算距离。
    pub fn model_at_distance(
        &self,
        key: EntityKey,
        distance: impl FnOnce() -> f32,
        bias: f32,
    ) -> Option<&str> {
        let mut levels = self
            .get::<LodGroup>(key)
            .into_iter()
            .flat_map(|lod| &lod.levels)
            .filter(|level| !level.model.is_empty());
        let Some(first) = levels.next() else {
            return self
                .get::<MeshRenderer>(key)
                .map(|mesh| mesh.model.as_str())
                .filter(|model| !model.is_empty());
        };
        let bias = if bias.is_finite() { bias } else { 1.0 };
        if bias <= 0.0 {
            return Some(first.model.as_str());
        }

        let distance = distance();
        let distance = if distance.is_finite() {
            distance.max(0.0)
        } else {
            0.0
        };
        let mut best: Option<(&str, f32)> = None;
        let mut unlimited = None;
        for level in std::iter::once(first).chain(levels) {
            let model = level.model.as_str();
            if level.max_distance.is_finite() && level.max_distance > 0.0 {
                let limit = level.max_distance * bias;
                if distance <= limit && best.is_none_or(|(_, current)| limit < current) {
                    best = Some((model, limit));
                }
            } else {
                unlimited.get_or_insert(model);
            }
        }
        best.map(|(model, _)| model).or(unlimited)
    }
}
