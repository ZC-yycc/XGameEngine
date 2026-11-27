//! 旧场景顶层结构；字段只在本 crate 内使用。

use serde::Deserialize;
use xgc_base::layer::LayerIndex;
use xgc_world::{CollisionLayer, ComponentData};

#[derive(Debug, Deserialize)]
pub(crate) struct OldScene {
    pub(crate) format_version: u32,
    #[serde(default)]
    #[allow(dead_code)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) entities: Vec<OldEntity>,
    /// 旧场景级光源表；缺字段时与旧 `Scene` 同款兜底（一盏默认方向光）。
    #[serde(default = "old_default_lights")]
    pub(crate) lights: Vec<OldLight>,
    #[serde(default = "old_default_ambient")]
    pub(crate) ambient: [f32; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub(crate) enum OldLightKind {
    #[default]
    Directional,
    Point,
}

/// 旧 `Light`：字段与缺省值必须逐项保持，迁移才只是搬运。
#[derive(Debug, Deserialize)]
pub(crate) struct OldLight {
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) kind: OldLightKind,
    #[serde(default = "old_default_direction")]
    pub(crate) direction: [f32; 3],
    #[serde(default)]
    pub(crate) position: [f32; 3],
    #[serde(default = "old_default_color")]
    pub(crate) color: [f32; 3],
    #[serde(default = "old_default_intensity")]
    pub(crate) intensity: f32,
    #[serde(default = "old_default_range")]
    pub(crate) range: f32,
    #[serde(default = "old_default_casts_shadow")]
    pub(crate) casts_shadow: bool,
    #[serde(default)]
    pub(crate) shadow_resolution: u32,
}

fn old_default_lights() -> Vec<OldLight> {
    vec![OldLight {
        name: "Directional Light".to_string(),
        kind: OldLightKind::Directional,
        direction: old_default_direction(),
        position: [0.0; 3],
        color: old_default_color(),
        intensity: old_default_intensity(),
        range: old_default_range(),
        casts_shadow: old_default_casts_shadow(),
        shadow_resolution: 0,
    }]
}

fn old_default_ambient() -> [f32; 3] {
    [0.06, 0.06, 0.08]
}

fn old_default_direction() -> [f32; 3] {
    [0.5, -1.0, 0.5]
}

fn old_default_color() -> [f32; 3] {
    [1.0, 0.98, 0.95]
}

fn old_default_intensity() -> f32 {
    3.0
}

fn old_default_range() -> f32 {
    10.0
}

fn old_default_casts_shadow() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub(crate) struct OldEntity {
    pub(crate) id: OldId,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) parent: Option<OldId>,
    /// 来源资产和文件内 ID 在项目迁移阶段一并重写。
    #[serde(default)]
    pub(crate) prefab: Option<OldPrefabRef>,
    #[serde(default)]
    pub(crate) layer: u8,
    pub(crate) transform: OldTransform,
    pub(crate) components: super::OldComponents,
}

impl OldEntity {
    pub(crate) fn layer_index(&self) -> Result<LayerIndex, String> {
        LayerIndex::new(self.layer).ok_or_else(|| {
            format!(
                "实体 {} 的 layer={} 超过 {}",
                self.id.0,
                self.layer,
                LayerIndex::MAX
            )
        })
    }

    pub(crate) fn layer_component(&self) -> Result<Option<ComponentData>, String> {
        let index = self.layer_index()?;
        Ok((index != LayerIndex::default())
            .then(|| ComponentData::from_component(&CollisionLayer { index })))
    }
}

/// 旧场景实体上的预制体实例引用。
#[derive(Debug, Deserialize)]
pub(crate) struct OldPrefabRef {
    pub(crate) source: String,
    pub(crate) source_id: OldId,
    pub(crate) is_root: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) struct OldId(pub(crate) u64);

#[derive(Debug, Deserialize)]
pub(crate) struct OldTransform {
    pub(crate) position: glam::Vec3,
    pub(crate) rotation: glam::Quat,
    pub(crate) scale: glam::Vec3,
}
