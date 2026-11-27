//! 将实例编辑投影到来源资产，场景变更由项目事务处理。

use std::collections::{BTreeMap, BTreeSet};

use xgc_base::EntityId;

use crate::{
    EntityData, GlobalTransform, Parent, PluginComponent, PrefabInstance, Script, Transform, World,
    WorldData,
};

use super::{find_root, instance_entities, read_component, rebuild_globals, remap_parent};
use crate::Component;

/// 去掉已删除的来源关联，保留实例的实体、父链、字段和修订号。
pub fn detach_prefab_sources(
    data: &mut WorldData,
    removed: &BTreeSet<EntityId>,
    mut matches_asset: impl FnMut(&str) -> Result<bool, String>,
) -> Result<bool, String> {
    let mut changed = false;
    for entity in &mut data.entities {
        let Some(index) = entity
            .components
            .iter()
            .position(|component| component.component == PrefabInstance::ID)
        else {
            continue;
        };
        let instance = read_component::<PrefabInstance>(&entity.components[index])?;
        if removed.contains(&EntityId::from_raw(instance.source_id))
            && matches_asset(&instance.source)?
        {
            entity.components.remove(index);
            changed = true;
        }
    }
    Ok(changed)
}

/// 按来源身份把实例的值投影回资产。场景局部 ID 与根摆放只属于实例。
pub fn apply_prefab_values(
    world: &World,
    prefab: &WorldData,
    selected: EntityId,
) -> Result<WorldData, String> {
    project_prefab(world, prefab, selected, None).map(|(asset, _)| asset)
}

/// 项目提交还可以删除完整的来源分支，并以返回的源 ID 集解除其他实例的关联。
pub fn apply_prefab_removals(
    world: &World,
    saved_scene: &WorldData,
    prefab: &WorldData,
    selected: EntityId,
) -> Result<(WorldData, BTreeSet<EntityId>), String> {
    project_prefab(world, prefab, selected, Some(saved_scene))
}

fn project_prefab(
    world: &World,
    prefab: &WorldData,
    selected: EntityId,
    saved_scene: Option<&WorldData>,
) -> Result<(WorldData, BTreeSet<EntityId>), String> {
    let (root, instance) = find_root(world, selected)?;
    let source_root = EntityId::from_raw(instance.source_id);
    let parents = asset_parents(world, prefab, source_root)?;
    let current = instance_entities(world, root, instance)?;
    let removed: BTreeSet<_> = parents
        .keys()
        .filter(|source| !current.contains_key(source))
        .copied()
        .collect();
    if current.keys().any(|source| !parents.contains_key(source)) {
        return Err("预制体实体的来源 ID 已变化，需要结构性 Apply".into());
    }
    if !removed.is_empty() {
        let saved_scene = saved_scene.ok_or("预制体实体结构已变化，需要结构性 Apply")?;
        if parents.iter().any(|(id, parent)| {
            !removed.contains(id) && parent.is_some_and(|parent| removed.contains(&parent))
        }) {
            return Err("结构性 Apply 删除的来源分支仍有存活子节点".into());
        }
        // 仅凭当前父链会把重挂接和解除关联错判为删除，须用同一实例的磁盘基准确认。
        let mut saved = World::new();
        saved.restore_world_data(saved_scene)?;
        let (saved_root, saved_instance) = find_root(&saved, root)?;
        if saved_root != root || saved_instance != instance {
            return Err("结构性 Apply 需要与磁盘同源的实例根".into());
        }
        let earlier = instance_entities(&saved, root, saved_instance)?;
        for source in &removed {
            if earlier
                .get(source)
                .is_none_or(|id| world.key(*id).is_some())
            {
                return Err(format!("来源实体 {source} 需要从已保存实例中删除"));
            }
        }
    }

    let source_ids: BTreeMap<_, _> = current.iter().map(|(&source, &id)| (id, source)).collect();
    let root_key = world.key(root).expect("实例根已定位");
    if let Some(parent) = world.get::<Parent>(root_key) {
        if source_ids.contains_key(&parent.entity) || world.key(parent.entity).is_none() {
            return Err(format!("预制体实例根的外部父实体无效：{}", parent.entity));
        }
    }
    let mut added = None;
    world.for_each::<Parent>(|key, parent| {
        if let Some(id) = world.entity(key) {
            if source_ids.contains_key(&parent.entity) && !source_ids.contains_key(&id) {
                added = Some(added.map_or(id, |previous: EntityId| previous.min(id)));
            }
        }
    });
    if let Some(id) = added {
        return Err(format!("预制体子树新增了实体 {id}，需要结构性 Apply"));
    }

    let mut candidate = prefab.clone();
    candidate
        .entities
        .retain(|entity| !removed.contains(&entity.id));
    for entity in &mut candidate.entities {
        let id = current[&entity.id];
        let mut incoming = world
            .entity_data(id)
            .ok_or_else(|| format!("无法读取实例实体：{id}"))?;
        let is_root = entity.id == source_root;
        incoming.components.retain(|component| {
            component.component != PrefabInstance::ID
                && !(is_root && component.component == Parent::ID)
        });
        remap_parent(&mut incoming, &source_ids);
        apply_components(entity, incoming, is_root)?;
    }
    if !removed.is_empty() {
        asset_parents(world, &candidate, source_root)?;
    }
    rebuild_globals(&mut candidate.entities, None, &[], Some(source_root))?;
    Ok((candidate, removed))
}

fn apply_components(
    target: &mut EntityData,
    incoming: EntityData,
    is_root: bool,
) -> Result<(), String> {
    let mut values: BTreeMap<_, _> = incoming
        .components
        .into_iter()
        .map(|component| (component.component, component))
        .collect();
    let structure_changed = || {
        format!(
            "预制体实体 {} 的组件结构已变化，需要结构性 Apply",
            target.id
        )
    };
    if target.components.len() != values.len() {
        return Err(structure_changed());
    }
    for component in &mut target.components {
        let value = values
            .remove(&component.component)
            .ok_or_else(&structure_changed)?;
        let same_key = match component.component {
            Script::ID => {
                read_component::<Script>(component)?.paths
                    == read_component::<Script>(&value)?.paths
            }
            PluginComponent::ID => {
                read_component::<PluginComponent>(component)?.registered_name
                    == read_component::<PluginComponent>(&value)?.registered_name
            }
            _ => true,
        };
        if !same_key {
            return Err(format!(
                "预制体实体 {} 的组件 {} 关联键已变化，需要结构性 Apply",
                target.id, component.component
            ));
        }
        // 场景的全局坐标与根的摆放不能写入资产。稍后用资产空间重算子节点缓存。
        if component.component != GlobalTransform::ID
            && !(is_root && component.component == Transform::ID)
        {
            component.fields = value.fields;
        }
    }
    Ok(())
}

fn asset_parents(
    world: &World,
    prefab: &WorldData,
    root: EntityId,
) -> Result<BTreeMap<EntityId, Option<EntityId>>, String> {
    let mut parents = BTreeMap::new();
    for entity in &prefab.entities {
        world.snapshot_from_data(entity)?;
        let mut parent = None;
        for component in &entity.components {
            match component.component {
                PrefabInstance::ID => return Err("预制体资产不能包含实例来源标记".into()),
                Parent::ID => parent = Some(read_component::<Parent>(component)?.entity),
                _ => {}
            }
        }
        if parents.insert(entity.id, parent).is_some() {
            return Err(format!("预制体实体 ID 重复：{}", entity.id));
        }
    }
    if parents.get(&root) != Some(&None) {
        return Err(format!("预制体资产没有匹配的根实体：{root}"));
    }
    let mut checked = BTreeSet::from([root]);
    let mut chain = Vec::new();
    for &start in parents.keys() {
        let mut id = start;
        while checked.insert(id) {
            chain.push(id);
            id = parents
                .get(&id)
                .copied()
                .flatten()
                .ok_or_else(|| format!("预制体实体 {start} 的父链没有到达资产根 {root}"))?;
        }
        if chain.contains(&id) {
            return Err(format!("预制体资产的父链存在循环：{id}"));
        }
        chain.clear();
    }
    Ok(parents)
}
