//! 预制体实例化：把 `WorldData` 里的实体分配新 ID 后一次性 Spawn。

use std::collections::BTreeMap;

use xgc_base::EntityId;
use xgc_world::{
    Command, Component, ComponentData, EntityData, PrefabInstance, Value, World, WorldData,
};

const PARENT_COMPONENT: u32 = 0x0104;

/// 返回旧 ID → 新 ID 的映射；父引用随映射重写。
pub fn instantiate_prefab(
    world: &mut World,
    prefab: &WorldData,
    source: Option<&str>,
) -> Result<Vec<(EntityId, EntityId)>, String> {
    if prefab.entities.is_empty() {
        return Err("预制体为空".to_string());
    }
    let start = world.reserve_entity_ids(prefab.entities.len() as u64);
    let mut mapping = BTreeMap::new();
    for (index, entity) in prefab.entities.iter().enumerate() {
        mapping.insert(entity.id, EntityId::from_raw(start + index as u64));
    }
    let mut commands = Vec::with_capacity(prefab.entities.len());
    for entity in &prefab.entities {
        let mut data = entity.clone();
        data.id = *mapping
            .get(&entity.id)
            .ok_or_else(|| format!("实体 {} 没有映射", entity.id.raw()))?;
        remap_references(&mut data, &mapping);
        if let Some(source) = source {
            data.components.push(ComponentData {
                component: PrefabInstance::ID,
                fields: vec![
                    ("source".to_string(), Value::String(source.to_string())),
                    ("source_id".to_string(), Value::Entity(entity.id.raw())),
                    (
                        "root".to_string(),
                        Value::Bool(
                            !entity
                                .components
                                .iter()
                                .any(|component| component.component.0 == PARENT_COMPONENT),
                        ),
                    ),
                ],
            });
        }
        commands.push(Command::Spawn { data });
    }
    let plan = world.plan(commands);
    if !plan.is_valid() {
        return Err(format!("预制体计划无效：{:?}", plan.diagnostics));
    }
    world
        .apply(&plan)
        .map_err(|diagnostics| format!("预制体应用失败：{diagnostics:?}"))?;
    Ok(mapping.into_iter().collect())
}

fn remap_references(data: &mut EntityData, mapping: &BTreeMap<EntityId, EntityId>) {
    for component in &mut data.components {
        if component.component.0 != PARENT_COMPONENT {
            continue;
        }
        for (field, value) in &mut component.fields {
            if field != "entity" {
                continue;
            }
            if let Value::Entity(raw) = value {
                if let Some(mapped) = mapping.get(&EntityId::from_raw(*raw)) {
                    *value = Value::Entity(mapped.raw());
                }
            }
        }
    }
}
