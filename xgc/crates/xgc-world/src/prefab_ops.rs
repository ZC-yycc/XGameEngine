//! 预制体实例化与还原，实体修改由世界命令事务统一提交。

use std::collections::{BTreeMap, HashMap, HashSet};

use xgc_base::EntityId;

use crate::{
    Command, Component, ComponentId, EntityData, GlobalTransform, Parent, PrefabInstance,
    Transform, Value, World, WorldData,
};

/// 为预制体分配新 ID，返回按资产 ID 排序的映射。
pub fn instantiate_prefab(
    world: &mut World,
    prefab: &WorldData,
    source: Option<&str>,
) -> Result<Vec<(EntityId, EntityId)>, String> {
    let mapping = map_entities(world, prefab, &BTreeMap::new())?;
    let commands = prefab
        .entities
        .iter()
        .map(|entity| {
            let mut data = entity.clone().with_id(mapping[&entity.id]);
            remap_parent(&mut data, &mapping);
            if let Some(source) = source {
                data.components
                    .push(instance_data(entity, source).into_data());
            }
            Command::Spawn { data }
        })
        .collect();
    apply(world, commands, "预制体")?;
    Ok(mapping.into_iter().collect())
}

/// 用映射重写父引用，保留指向映射外实体的引用。
pub fn remap_parent(data: &mut EntityData, mapping: &BTreeMap<EntityId, EntityId>) {
    for component in &mut data.components {
        if component.component != Parent::ID {
            continue;
        }
        for (field, value) in &mut component.fields {
            if let ("entity", Value::Entity(raw)) = (field.as_str(), value) {
                if let Some(mapped) = mapping.get(&EntityId::from_raw(*raw)) {
                    *raw = mapped.raw();
                }
            }
        }
    }
}

/// 仅还原选中实体所属的实例，返回按资产 ID 排序的映射。
/// 已有实体按来源 ID 保留场景 ID 和变换，实例根保留场景中的外部父节点。
pub fn revert_prefab(
    world: &mut World,
    prefab: &WorldData,
    selected: EntityId,
) -> Result<Vec<(EntityId, EntityId)>, String> {
    let (root, instance) = find_root(world, selected)?;
    let current = instance_entities(world, root, instance)?;
    let mapping = map_entities(world, prefab, &current)?;
    let old_root_source = EntityId::from_raw(instance.source_id);
    let mut asset_roots = prefab.entities.iter().filter(|entity| !has_parent(entity));
    let first_root = asset_roots
        .next()
        .ok_or_else(|| "预制体没有根实体".to_string())?;
    let retained_root = if first_root.id == old_root_source {
        first_root
    } else {
        asset_roots
            .find(|entity| entity.id == old_root_source)
            .unwrap_or(first_root)
    };
    let root_data = world
        .entity_data(root)
        .ok_or_else(|| format!("无法读取预制体实例根：{root}"))?;
    let mut commands = Vec::new();
    for entity in &prefab.entities {
        let mut data = entity.clone().with_id(mapping[&entity.id]);
        remap_parent(&mut data, &mapping);
        data.components
            .retain(|component| component.component != PrefabInstance::ID);
        data.components
            .push(instance_data(entity, &instance.source).into_data());
        if entity.id == retained_root.id {
            preserve_component(&mut data, &root_data, Parent::ID);
        }
        if let Some(existing_id) = current.get(&entity.id) {
            let existing = world
                .entity_data(*existing_id)
                .ok_or_else(|| format!("无法读取预制体实例实体：{existing_id}"))?;
            preserve_component(&mut data, &existing, Transform::ID);
            preserve_component(&mut data, &existing, GlobalTransform::ID);
            commands.push(Command::Replace { data });
        } else {
            commands.push(Command::Spawn { data });
        }
    }
    for (source_id, entity) in current {
        if !mapping.contains_key(&source_id) {
            commands.push(Command::Despawn { entity });
        }
    }
    apply(world, commands, "预制体 Revert")?;
    Ok(mapping.into_iter().collect())
}

fn map_entities(
    world: &World,
    prefab: &WorldData,
    current: &BTreeMap<EntityId, EntityId>,
) -> Result<BTreeMap<EntityId, EntityId>, String> {
    if prefab.entities.is_empty() {
        return Err("预制体为空".to_string());
    }
    // Spawn 提交时会推进世界的 ID 分配器，坏计划无需预留 ID。
    let mut available = world.next_id..u64::MAX;
    let mut mapping = BTreeMap::new();
    for entity in &prefab.entities {
        let target = match current.get(&entity.id) {
            Some(&id) => id,
            None => available
                .next()
                .map(EntityId::from_raw)
                .ok_or_else(|| "预制体所需的实体 ID 已耗尽".to_string())?,
        };
        if mapping.insert(entity.id, target).is_some() {
            return Err(format!("预制体实体 ID 重复：{}", entity.id));
        }
    }
    Ok(mapping)
}

fn find_root(world: &World, selected: EntityId) -> Result<(EntityId, &PrefabInstance), String> {
    let source = world
        .key(selected)
        .and_then(|key| world.get::<PrefabInstance>(key))
        .ok_or_else(|| format!("所选实体不属于预制体实例：{selected}"))?
        .source
        .as_str();
    let mut entity = selected;
    let mut visited = HashSet::new();
    while visited.insert(entity) {
        let Some(key) = world.key(entity) else { break };
        let Some(instance) = world.get::<PrefabInstance>(key) else {
            break;
        };
        if instance.source != source {
            break;
        }
        if instance.root {
            return Ok((entity, instance));
        }
        let Some(parent) = world.get::<Parent>(key) else {
            break;
        };
        entity = parent.entity;
    }
    Err(format!("找不到所选实体的预制体实例根：{selected}"))
}

fn instance_entities(
    world: &World,
    root: EntityId,
    instance: &PrefabInstance,
) -> Result<BTreeMap<EntityId, EntityId>, String> {
    let mut children: HashMap<EntityId, Vec<(EntityId, EntityId)>> = HashMap::new();
    // 即便两份同源实例互为父子，另一个实例根也会截断遍历。
    world.for_each2::<PrefabInstance, Parent>(|key, child, parent| {
        if !child.root && child.source == instance.source {
            if let Some(id) = world.entity(key) {
                children
                    .entry(parent.entity)
                    .or_default()
                    .push((EntityId::from_raw(child.source_id), id));
            }
        }
    });
    let mut pending = vec![(EntityId::from_raw(instance.source_id), root)];
    let mut current = BTreeMap::new();
    while let Some((source_id, id)) = pending.pop() {
        if current.insert(source_id, id).is_some() {
            return Err(format!("预制体实例内的来源实体 ID 重复：{source_id}"));
        }
        if let Some(children) = children.remove(&id) {
            pending.extend(children);
        }
    }
    Ok(current)
}

fn instance_data(entity: &EntityData, source: &str) -> PrefabInstance {
    PrefabInstance {
        source: source.to_string(),
        source_id: entity.id.raw(),
        root: !has_parent(entity),
    }
}

fn has_parent(data: &EntityData) -> bool {
    data.components
        .iter()
        .any(|component| component.component == Parent::ID)
}

fn preserve_component(data: &mut EntityData, existing: &EntityData, component_id: ComponentId) {
    let Some(component) = existing
        .components
        .iter()
        .find(|component| component.component == component_id)
    else {
        return;
    };
    if let Some(slot) = data
        .components
        .iter_mut()
        .find(|slot| slot.component == component_id)
    {
        *slot = component.clone();
    } else {
        data.components.push(component.clone());
    }
}

fn apply(world: &mut World, commands: Vec<Command>, operation: &str) -> Result<(), String> {
    let plan = world.plan(commands);
    if !plan.is_valid() {
        return Err(format!("{operation}计划无效：{:?}", plan.diagnostics));
    }
    let next_id = world.next_id;
    world.apply(&plan).map(|_| ()).map_err(|diagnostics| {
        world.next_id = next_id;
        format!("{operation}应用失败：{diagnostics:?}")
    })
}
