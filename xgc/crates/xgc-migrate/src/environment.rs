//! 场景级环境（光 / 环境光）的迁移：旧引擎把它们放在场景顶层，不是实体。

use xgc_world::{LightKind, SceneEnvironment, SceneLight};

use crate::old_scene::{OldLight, OldLightKind, OldScene};

/// 场景级光 / 环境光：字段与缺省值都不改写，只搬位置。
pub(crate) fn scene_environment(scene: &OldScene) -> SceneEnvironment {
    SceneEnvironment {
        lights: scene.lights.iter().map(scene_light).collect(),
        ambient: scene.ambient,
    }
}

fn scene_light(light: &OldLight) -> SceneLight {
    SceneLight {
        name: light.name.clone(),
        kind: match light.kind {
            OldLightKind::Directional => LightKind::Directional,
            OldLightKind::Point => LightKind::Point,
        },
        direction: light.direction,
        position: light.position,
        color: light.color,
        intensity: light.intensity,
        range: light.range,
        casts_shadow: light.casts_shadow,
        shadow_resolution: light.shadow_resolution,
    }
}
