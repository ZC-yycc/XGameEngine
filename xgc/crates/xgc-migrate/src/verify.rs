//! 迁移验收：把旧场景重新迁移的结果与目标 `WorldData` 逐实体 / 逐字段对比。

use std::collections::BTreeMap;

use xgc_base::EntityId;
use xgc_world::{ComponentData, EntityData, SceneEnvironment, WorldData};

use crate::migrate_str;

/// 一致时返回 `Ok(())`；否则返回可读差异列表。
pub fn verify_migration(scene: &str, actual: &WorldData) -> Result<(), Vec<String>> {
    let (expected, _report) =
        migrate_str(scene).map_err(|err| vec![format!("旧场景迁移失败：{err:?}")])?;
    let mut differences = Vec::new();
    if expected.next_id != actual.next_id {
        differences.push(format!(
            "next_id 期望 {}，实际 {}",
            expected.next_id, actual.next_id
        ));
    }
    compare_environment(&expected.environment, &actual.environment, &mut differences);
    let expected_entities = by_id(&expected.entities);
    let actual_entities = by_id(&actual.entities);
    for (id, expected) in &expected_entities {
        match actual_entities.get(id) {
            None => differences.push(format!("缺少实体 {id}")),
            Some(actual) => compare_entity(*id, expected, actual, &mut differences),
        }
    }
    for id in actual_entities.keys() {
        if !expected_entities.contains_key(id) {
            differences.push(format!("多出实体 {id}"));
        }
    }
    if differences.is_empty() {
        Ok(())
    } else {
        Err(differences)
    }
}

fn by_id(entities: &[EntityData]) -> BTreeMap<EntityId, &EntityData> {
    entities.iter().map(|entity| (entity.id, entity)).collect()
}

/// 环境是场景级数据，不挂在实体上，必须单独对照——否则光源丢失不会被发现。
fn compare_environment(
    expected: &SceneEnvironment,
    actual: &SceneEnvironment,
    differences: &mut Vec<String>,
) {
    if !expected
        .ambient
        .iter()
        .zip(&actual.ambient)
        .all(|(expected, actual)| close(*expected, *actual))
    {
        differences.push(format!(
            "ambient 期望 {:?}，实际 {:?}",
            expected.ambient, actual.ambient
        ));
    }
    if expected.lights.len() != actual.lights.len() {
        differences.push(format!(
            "光源数量期望 {}，实际 {}",
            expected.lights.len(),
            actual.lights.len()
        ));
    }
    for (index, (expected, actual)) in expected.lights.iter().zip(&actual.lights).enumerate() {
        let mismatched = [
            ("name", expected.name != actual.name),
            ("kind", expected.kind != actual.kind),
            ("casts_shadow", expected.casts_shadow != actual.casts_shadow),
            (
                "shadow_resolution",
                expected.shadow_resolution != actual.shadow_resolution,
            ),
            (
                "direction",
                !close_vec3(expected.direction, actual.direction),
            ),
            ("position", !close_vec3(expected.position, actual.position)),
            ("color", !close_vec3(expected.color, actual.color)),
            ("intensity", !close(expected.intensity, actual.intensity)),
            ("range", !close(expected.range, actual.range)),
        ]
        .into_iter()
        .find(|(_, mismatch)| *mismatch);
        if let Some((field, _)) = mismatched {
            differences.push(format!("光源 {index} 字段 {field} 与旧场景不一致"));
        }
    }
}

fn close_vec3(expected: [f32; 3], actual: [f32; 3]) -> bool {
    expected
        .iter()
        .zip(&actual)
        .all(|(expected, actual)| close(*expected, *actual))
}

fn compare_entity(
    id: EntityId,
    expected: &EntityData,
    actual: &EntityData,
    differences: &mut Vec<String>,
) {
    let expected_components = by_component(&expected.components);
    let actual_components = by_component(&actual.components);
    for (component_id, expected) in &expected_components {
        match actual_components.get(component_id) {
            None => differences.push(format!("实体 {id} 缺少组件 {component_id}")),
            Some(actual) => compare_component(id, *component_id, expected, actual, differences),
        }
    }
    for component_id in actual_components.keys() {
        if !expected_components.contains_key(component_id) {
            differences.push(format!("实体 {id} 多出组件 {component_id}"));
        }
    }
}

fn by_component(components: &[ComponentData]) -> BTreeMap<u32, &ComponentData> {
    components
        .iter()
        .map(|component| (component.component.0, component))
        .collect()
}

fn compare_component(
    id: EntityId,
    component: u32,
    expected: &ComponentData,
    actual: &ComponentData,
    differences: &mut Vec<String>,
) {
    let expected_fields: BTreeMap<&str, _> = expected
        .fields
        .iter()
        .map(|(name, value)| (name.as_str(), value))
        .collect();
    let actual_fields: BTreeMap<&str, _> = actual
        .fields
        .iter()
        .map(|(name, value)| (name.as_str(), value))
        .collect();
    for (name, expected) in &expected_fields {
        match actual_fields.get(name) {
            None => differences.push(format!("实体 {id} 组件 {component} 缺少字段 {name}")),
            Some(actual) if !values_close(expected, actual) => differences.push(format!(
                "实体 {id} 组件 {component} 字段 {name} 期望 {expected:?}，实际 {actual:?}"
            )),
            Some(_) => {}
        }
    }
    for name in actual_fields.keys() {
        if !expected_fields.contains_key(name) {
            differences.push(format!("实体 {id} 组件 {component} 多出字段 {name}"));
        }
    }
}

fn values_close(expected: &xgc_world::Value, actual: &xgc_world::Value) -> bool {
    use xgc_world::Value;
    match (expected, actual) {
        (Value::F64(a), Value::F64(b)) => close(*a as f32, *b as f32),
        (Value::Vec2(a), Value::Vec2(b)) => {
            a.iter().zip(b).all(|(a, b)| close(*a as f32, *b as f32))
        }
        (Value::Vec3(a), Value::Vec3(b)) => {
            a.iter().zip(b).all(|(a, b)| close(*a as f32, *b as f32))
        }
        (Value::Vec4(a), Value::Vec4(b)) | (Value::Quat(a), Value::Quat(b)) => {
            a.iter().zip(b).all(|(a, b)| close(*a as f32, *b as f32))
        }
        (Value::I64(a), Value::I64(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Entity(a), Value::Entity(b)) => a == b,
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| values_close(a, b))
        }
        (Value::Struct(a), Value::Struct(b)) | (Value::Map(a), Value::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|((a_key, a), (b_key, b))| a_key == b_key && values_close(a, b))
        }
        (
            Value::Enum {
                variant: a_variant,
                payload: a,
            },
            Value::Enum {
                variant: b_variant,
                payload: b,
            },
        ) => a_variant == b_variant && values_close(a, b),
        _ => false,
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
}
