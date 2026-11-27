//! 预制体实例化、还原与值写回，共用来源身份和父链映射。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use xgc_base::EntityId;

use crate::{
    Command, Component, ComponentData, EntityData, GlobalTransform, Parent, PluginComponent,
    PrefabInstance, Script, Transform, Value, World, WorldData,
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
                    .push(ComponentData::from_component(&instance_data(
                        entity, source,
                    )));
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
/// 已有实体按来源 ID 保留场景 ID；实例根保留场景中的局部变换和外部父节点。
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
    let mut entities = Vec::with_capacity(prefab.entities.len());
    for entity in &prefab.entities {
        let mut data = entity.clone().with_id(mapping[&entity.id]);
        remap_parent(&mut data, &mapping);
        data.components
            .retain(|component| component.component != PrefabInstance::ID);
        data.components
            .push(ComponentData::from_component(&instance_data(
                entity,
                &instance.source,
            )));
        if entity.id == retained_root.id {
            // 同一个外部父节点下保留局部 TRS，即可精确保留摆放，无需分解世界矩阵。
            data.components
                .retain(|component| component.component != Transform::ID);
            data.components.extend(
                root_data
                    .components
                    .iter()
                    .filter(|component| matches!(component.component, Parent::ID | Transform::ID))
                    .cloned(),
            );
        }
        entities.push(data);
    }
    let removed: Vec<_> = current
        .into_iter()
        .filter_map(|(source_id, entity)| (!mapping.contains_key(&source_id)).then_some(entity))
        .collect();
    rebuild_globals(&mut entities, Some(world), &removed, None)?;
    let commands = entities
        .into_iter()
        .map(|data| {
            if world.key(data.id).is_some() {
                Command::Replace { data }
            } else {
                Command::Spawn { data }
            }
        })
        .chain(
            removed
                .into_iter()
                .map(|entity| Command::Despawn { entity }),
        )
        .collect();
    apply(world, commands, "预制体 Revert")?;
    Ok(mapping.into_iter().collect())
}

/// 按来源身份把实例的值投影回资产。场景局部 ID 与根摆放只属于实例。
pub fn apply_prefab_values(
    world: &World,
    prefab: &WorldData,
    selected: EntityId,
) -> Result<WorldData, String> {
    let (root, instance) = find_root(world, selected)?;
    let source_root = EntityId::from_raw(instance.source_id);
    validate_apply_asset(world, prefab, source_root)?;
    let current = instance_entities(world, root, instance)?;
    if current.len() != prefab.entities.len() {
        return Err("预制体实体结构已变化，需要结构性 Apply".into());
    }
    let source_ids: BTreeMap<_, _> = current.iter().map(|(&source, &id)| (id, source)).collect();
    let root_key = world.key(root).expect("实例根已定位");
    if let Some(parent) = world.get::<Parent>(root_key) {
        if source_ids.contains_key(&parent.entity) || world.key(parent.entity).is_none() {
            return Err(format!("预制体实例根的外部父实体无效：{}", parent.entity));
        }
    }
    // 本地子节点和另一实例的根没有本次 Apply 的来源身份。
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
    for entity in &mut candidate.entities {
        let id = current.get(&entity.id).copied().ok_or_else(|| {
            format!(
                "预制体实体 {} 的来源 ID 已变化，需要结构性 Apply",
                entity.id
            )
        })?;
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
    rebuild_globals(&mut candidate.entities, None, &[], Some(source_root))?;
    Ok(candidate)
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

fn validate_apply_asset(world: &World, prefab: &WorldData, root: EntityId) -> Result<(), String> {
    let mut parents = BTreeMap::new();
    for entity in &prefab.entities {
        world.snapshot_from_data(entity)?;
        if entity.id.raw() == u64::MAX {
            return Err("预制体实体 ID 无效".into());
        }
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
    Ok(())
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

#[derive(Clone, Copy)]
struct LocalPose {
    transform: Transform,
    parent: Option<EntityId>,
}

fn rebuild_globals(
    entities: &mut [EntityData],
    world: Option<&World>,
    removed: &[EntityId],
    preserved_root: Option<EntityId>,
) -> Result<(), String> {
    // 视口直接读全局变换，场景实例与资产各自按所在空间的父链重算。
    let mut globals = HashMap::new();
    let mut poses: HashMap<_, _> = removed.iter().map(|&id| (id, None)).collect();
    for entity in entities.iter() {
        let mut has_global = false;
        let mut transform = Transform::default();
        let mut parent = None;
        for component in &entity.components {
            match component.component {
                GlobalTransform::ID => {
                    // 覆写派生值前仍校验原字段，让损坏的资产不能借编辑隐藏。
                    let global = read_component::<GlobalTransform>(component)?;
                    if Some(entity.id) == preserved_root {
                        globals.insert(entity.id, global);
                    }
                    has_global = true;
                }
                Transform::ID => transform = read_component(component)?,
                Parent::ID => parent = Some(read_component::<Parent>(component)?.entity),
                _ => {}
            }
        }
        poses.insert(
            entity.id,
            has_global.then_some(LocalPose { transform, parent }),
        );
    }

    let mut chain = Vec::new();
    let mut visited = HashSet::new();
    for entity in entities {
        if Some(entity.id) == preserved_root {
            continue;
        }
        let Some(component) = entity
            .components
            .iter_mut()
            .find(|component| component.component == GlobalTransform::ID)
        else {
            continue;
        };
        visited.clear();
        let mut ancestor = Some(entity.id);
        let mut parent_global = None;
        while let Some(id) = ancestor {
            if let Some(&global) = globals.get(&id) {
                parent_global = Some(global);
                break;
            }
            let pose = match poses.get(&id) {
                Some(&pose) => pose,
                None => world.and_then(|world| live_pose(world, id)),
            };
            let Some(pose) = pose else { break };
            if !visited.insert(id) {
                return Err(format!("预制体父链存在循环：{id}"));
            }
            chain.push((id, pose.transform));
            ancestor = pose.parent;
        }
        while let Some((id, local)) = chain.pop() {
            let global = match parent_global {
                Some(parent) => GlobalTransform::compose(&parent, &local),
                None => GlobalTransform::from_local(&local),
            };
            globals.insert(id, global);
            parent_global = Some(global);
        }

        let global = &globals[&entity.id];
        for field in GlobalTransform::fields() {
            let value = global.get_field(field.name).expect("全局变换字段必须可读");
            if let Some((_, slot)) = component
                .fields
                .iter_mut()
                .find(|(name, _)| name == field.name)
            {
                *slot = value;
            } else {
                component.fields.push((field.name.to_string(), value));
            }
        }
    }
    Ok(())
}

fn read_component<T: Component>(data: &ComponentData) -> Result<T, String> {
    let mut component = T::default_value();
    for (name, value) in &data.fields {
        component.set_field(name, value)?;
    }
    Ok(component)
}

fn live_pose(world: &World, id: EntityId) -> Option<LocalPose> {
    let key = world.key(id)?;
    world.get::<GlobalTransform>(key)?;
    Some(LocalPose {
        transform: world.get::<Transform>(key).copied().unwrap_or_default(),
        parent: world.get::<Parent>(key).map(|parent| parent.entity),
    })
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
