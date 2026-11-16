//! 预制体实例化：把 `WorldData` 里的实体分配新 ID 后一次性 Spawn。

use std::collections::BTreeMap;

use xgc_base::EntityId;
use xgc_world::{
    Command, Component, ComponentData, EntityData, PrefabInstance, Value, World, WorldData,
};

const PARENT_COMPONENT: u32 = 0x0104;
const PREFAB_COMPONENT: u32 = 0x0802;

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

/// 预制体 Revert：按 `prefab.instance.source_id` 认回实例实体。
///
/// - 已存在实体保留 `EntityId` 与局部 / 全局 Transform；
/// - 资产新增实体分配新 ID；
/// - 资产删掉的实体 Despawn；
/// - 父引用按新旧映射重写。
pub fn revert_prefab(
    world: &mut World,
    prefab: &WorldData,
    source: &str,
) -> Result<Vec<(EntityId, EntityId)>, String> {
    let snapshot = world.world_data();
    let mut current = BTreeMap::new();
    for entity in &snapshot.entities {
        if let Some((instance_source, source_id)) = instance_meta(entity) {
            if instance_source == source {
                current.insert(source_id, entity.id);
            }
        }
    }
    let prefab_ids: Vec<u64> = prefab
        .entities
        .iter()
        .map(|entity| entity.id.raw())
        .collect();
    let missing: Vec<u64> = prefab_ids
        .iter()
        .copied()
        .filter(|id| !current.contains_key(id))
        .collect();
    let start = world.reserve_entity_ids(missing.len() as u64);
    let mut targets = current.clone();
    for (index, source_id) in missing.iter().enumerate() {
        targets.insert(*source_id, EntityId::from_raw(start + index as u64));
    }
    let mut mapping = BTreeMap::new();
    for (source_id, target) in &targets {
        if prefab_ids.contains(source_id) {
            mapping.insert(EntityId::from_raw(*source_id), *target);
        }
    }

    let mut commands = Vec::new();
    for entity in &prefab.entities {
        let source_id = entity.id.raw();
        let target = *targets
            .get(&source_id)
            .ok_or_else(|| format!("预制体实体 {source_id} 没有目标 ID"))?;
        let mut data = entity.clone().with_id(target);
        remap_references(&mut data, &mapping);
        if let Some(existing_id) = current.get(&source_id) {
            if let Some(existing) = world.entity_data(*existing_id) {
                preserve_component(&mut data, &existing, 0x0101);
                preserve_component(&mut data, &existing, 0x0105);
            }
        }
        set_instance_meta(&mut data, source, source_id, !has_parent(entity));
        if current.contains_key(&source_id) {
            commands.push(Command::Replace { data });
        } else {
            commands.push(Command::Spawn { data });
        }
    }
    for (source_id, entity) in &current {
        if !prefab_ids.contains(source_id) {
            commands.push(Command::Despawn { entity: *entity });
        }
    }
    let plan = world.plan(commands);
    if !plan.is_valid() {
        return Err(format!("预制体 Revert 计划无效：{:?}", plan.diagnostics));
    }
    world
        .apply(&plan)
        .map_err(|diagnostics| format!("预制体 Revert 失败：{diagnostics:?}"))?;
    Ok(mapping.into_iter().collect())
}

fn instance_meta(data: &EntityData) -> Option<(String, u64)> {
    let component = data
        .components
        .iter()
        .find(|component| component.component.0 == PREFAB_COMPONENT)?;
    let mut source = None;
    let mut source_id = None;
    for (field, value) in &component.fields {
        match (field.as_str(), value) {
            ("source", Value::String(value)) => source = Some(value.clone()),
            ("source_id", Value::Entity(value)) => source_id = Some(*value),
            _ => {}
        }
    }
    Some((source?, source_id?))
}

fn set_instance_meta(data: &mut EntityData, source: &str, source_id: u64, root: bool) {
    data.components
        .retain(|component| component.component.0 != PREFAB_COMPONENT);
    data.components.push(ComponentData {
        component: PrefabInstance::ID,
        fields: vec![
            ("source".to_string(), Value::String(source.to_string())),
            ("source_id".to_string(), Value::Entity(source_id)),
            ("root".to_string(), Value::Bool(root)),
        ],
    });
}

fn preserve_component(data: &mut EntityData, existing: &EntityData, component_id: u32) {
    let Some(component) = existing
        .components
        .iter()
        .find(|component| component.component.0 == component_id)
    else {
        return;
    };
    if let Some(slot) = data
        .components
        .iter_mut()
        .find(|slot| slot.component.0 == component_id)
    {
        *slot = component.clone();
    } else {
        data.components.push(component.clone());
    }
}

fn has_parent(data: &EntityData) -> bool {
    data.components
        .iter()
        .any(|component| component.component.0 == PARENT_COMPONENT)
}
