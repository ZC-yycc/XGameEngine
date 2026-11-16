//! 旧 `Transform` → XGC `Transform` + `GlobalTransform`（同值，层级系统随后重算）。

use xgc_world::{Component, ComponentData, GlobalTransform, Transform, Value};

pub(crate) fn transform_components(
    position: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
) -> (ComponentData, ComponentData) {
    let transform = ComponentData {
        component: Transform::ID,
        fields: vec![
            (
                "position".to_string(),
                Value::Vec3([
                    f64::from(position[0]),
                    f64::from(position[1]),
                    f64::from(position[2]),
                ]),
            ),
            (
                "rotation".to_string(),
                Value::Quat([
                    f64::from(rotation[0]),
                    f64::from(rotation[1]),
                    f64::from(rotation[2]),
                    f64::from(rotation[3]),
                ]),
            ),
            (
                "scale".to_string(),
                Value::Vec3([
                    f64::from(scale[0]),
                    f64::from(scale[1]),
                    f64::from(scale[2]),
                ]),
            ),
        ],
    };
    let global = ComponentData {
        component: GlobalTransform::ID,
        fields: transform.fields.clone(),
    };
    (transform, global)
}
