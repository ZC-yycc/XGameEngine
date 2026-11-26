//! 只读诊断使用渲染器的模型抽取判据。

use serde_json::{json, Value as Json};
use xgc_base::EntityId;
use xgc_render::{camera_view_projection_with_eye, explain_draw_model, ModelExtractionFailure};
use xgc_world::{Camera, GlobalTransform, World};

use crate::required_u64;

pub(super) const DEFAULT_LOD_BIAS: f32 = 1.0;

pub(super) fn explain(world: &World, params: &Json) -> Result<Json, String> {
    let id = EntityId::from_raw(required_u64(params, "id")?);
    let key = world.key(id).ok_or_else(|| format!("未知实体 id：{id}"))?;
    let eye = match (provided(params, "camera"), provided(params, "eye")) {
        (Some(_), Some(_)) => return Err("camera 与 eye 只能提供其中一个".into()),
        (Some(_), None) => explicit_camera_eye(world, params)?,
        (None, Some(eye)) => {
            let Some([x, y, z]) = eye.as_array().map(Vec::as_slice) else {
                return Err("eye 必须包含三个坐标".into());
            };
            [
                finite_float(x, "eye")?,
                finite_float(y, "eye")?,
                finite_float(z, "eye")?,
            ]
        }
        // 眼点与投影尺寸无关；复用渲染器实际使用的相机选择。
        (None, None) => {
            camera_view_projection_with_eye(world, 1, 1).map_or([0.0; 3], |(_, eye)| eye)
        }
    };
    let lod_bias = provided(params, "lod_bias").map_or(Ok(DEFAULT_LOD_BIAS), |value| {
        finite_float(value, "lod_bias")
    })?;
    let (model, reason) = match explain_draw_model(world, key, eye, lod_bias) {
        Ok(model) => (Some(model), None),
        Err(reason) => (None, Some(reason_code(reason))),
    };
    Ok(json!({
        "entity": id,
        "render": {
            "stage": "model_extraction",
            "included": model.is_some(),
            "model": model,
            "reason": reason,
            "eye": eye,
            "lod_bias": lod_bias,
        },
    }))
}

fn explicit_camera_eye(world: &World, params: &Json) -> Result<[f32; 3], String> {
    let id = EntityId::from_raw(required_u64(params, "camera")?);
    let key = world
        .key(id)
        .ok_or_else(|| format!("未知 camera 实体：{id}"))?;
    if world.get::<Camera>(key).is_none() {
        return Err(format!("camera 实体 {id} 缺少 render.camera"));
    }
    world
        .get::<GlobalTransform>(key)
        .map(|transform| transform.position)
        .ok_or_else(|| format!("camera 实体 {id} 缺少 core.global_transform"))
}

fn provided<'a>(params: &'a Json, name: &str) -> Option<&'a Json> {
    params.get(name).filter(|value| !value.is_null())
}

fn finite_float(value: &Json, name: &str) -> Result<f32, String> {
    let max = f64::from(f32::MAX);
    value
        .as_f64()
        .filter(|number| (-max..=max).contains(number))
        .map(|number| number as f32)
        .ok_or_else(|| format!("{name} 必须是有限的 f32 数值"))
}

fn reason_code(reason: ModelExtractionFailure) -> &'static str {
    match reason {
        ModelExtractionFailure::MissingModelComponent => "missing_model_component",
        ModelExtractionFailure::MissingGlobalTransform => "missing_global_transform",
        ModelExtractionFailure::EmptyModel => "empty_model",
        ModelExtractionFailure::OutsideLodDistance => "outside_lod_distance",
    }
}
