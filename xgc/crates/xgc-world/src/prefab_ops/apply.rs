//! 将实例编辑投影到来源资产，场景变更由项目事务处理。

use std::collections::{BTreeMap, BTreeSet};

use xgc_base::EntityId;

use crate::{
    EntityData, GlobalTransform, Parent, PluginComponent, PrefabInstance, Script, Transform, Value,
    World, WorldData,
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
    project_prefab(world, prefab, selected, None).map(|plan| plan.asset)
}

/// 资产与实例关联的投影。只有验证完的单叶新增才需要向项目领号。
pub struct PrefabApplyPlan {
    asset: WorldData,
    removed: BTreeSet<EntityId>,
    new_leaf: Option<NewLeaf>,
}

struct NewLeaf {
    local: EntityId,
    source: String,
    minimum: EntityId,
}

impl PrefabApplyPlan {
    pub fn removed_sources(&self) -> &BTreeSet<EntityId> {
        &self.removed
    }

    /// 需要为资产新增的叶节点领号时，给出分配下限。
    pub fn source_id_minimum(&self) -> Option<EntityId> {
        self.new_leaf.as_ref().map(|leaf| leaf.minimum)
    }

    /// 用项目已发行的来源 ID 完成资产，返回需要写回当前草稿的关联。
    pub fn finish(
        mut self,
        source_id: Option<EntityId>,
    ) -> Result<(WorldData, Option<(EntityId, PrefabInstance)>), String> {
        let binding = match (self.new_leaf, source_id) {
            (None, None) => None,
            (Some(leaf), Some(id)) if id >= leaf.minimum && id.validate().is_ok() => {
                // 只有新行使用占位 ID，任何父引用均已指向已有来源实体。
                self.asset.entities.last_mut().expect("已投影新增叶节点").id = id;
                self.asset.next_id = id.raw() + 1;
                Some((
                    leaf.local,
                    PrefabInstance {
                        source: leaf.source,
                        source_id: id.raw(),
                        root: false,
                    },
                ))
            }
            _ => return Err("新增预制体叶节点与项目发行的实体 ID 不匹配".into()),
        };
        Ok((self.asset, binding))
    }
}

/// 以已保存的实例为结构基准，共用值投影并检查新增或删除所需的身份。
pub fn plan_prefab_apply(
    world: &World,
    saved_scene: &WorldData,
    prefab: &WorldData,
    selected: EntityId,
) -> Result<PrefabApplyPlan, String> {
    project_prefab(world, prefab, selected, Some(saved_scene))
}

fn project_prefab(
    world: &World,
    prefab: &WorldData,
    selected: EntityId,
    saved_scene: Option<&WorldData>,
) -> Result<PrefabApplyPlan, String> {
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
    if !removed.is_empty() && saved_scene.is_none() {
        return Err("预制体实体结构已变化，需要结构性 Apply".into());
    }

    let source_ids: BTreeMap<_, _> = current.iter().map(|(&source, &id)| (id, source)).collect();
    let root_key = world.key(root).expect("实例根已定位");
    if let Some(parent) = world.get::<Parent>(root_key) {
        if source_ids.contains_key(&parent.entity) || world.key(parent.entity).is_none() {
            return Err(format!("预制体实例根的外部父实体无效：{}", parent.entity));
        }
    }
    let mut added = BTreeSet::new();
    world.for_each::<Parent>(|key, parent| {
        if let Some(id) = world.entity(key) {
            if source_ids.contains_key(&parent.entity) && !source_ids.contains_key(&id) {
                added.insert(id);
            }
        }
    });
    let mut new_leaf = None;
    if !removed.is_empty() || !added.is_empty() {
        let saved_scene = saved_scene.ok_or_else(|| {
            format!(
                "预制体子树新增了实体 {}，需要结构性 Apply",
                added.first().expect("存在新增实体")
            )
        })?;
        // 只看当前父链会把重挂接和 Undo 解除来源关联误判成删除及新增。
        let mut saved = World::new();
        saved.restore_world_data(saved_scene)?;
        let (saved_root, saved_instance) = find_root(&saved, root)?;
        if saved_root != root || saved_instance != instance {
            return Err("结构性 Apply 需要与磁盘同源的实例根".into());
        }
        let earlier = instance_entities(&saved, root, saved_instance)?;
        if let Some(&id) = added.first() {
            if !removed.is_empty() {
                return Err("来源关联有缺失，不能新增叶节点。刚撤销的关联可以通过重做恢复".into());
            }
            if earlier != current {
                return Err("新增叶节点需要已保存的完整实例，请先保存草稿中的来源关联".into());
            }
            if added.len() != 1 {
                return Err("结构性 Apply 一次只能新增一个叶节点".into());
            }
            new_leaf = Some(project_leaf(
                world,
                prefab,
                instance,
                &parents,
                &source_ids,
                id,
            )?);
        } else {
            validate_removals(world, &parents, &removed, &earlier)?;
        }
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
    let new_leaf = new_leaf.map(|(leaf, row)| {
        candidate.entities.push(row);
        leaf
    });
    if !removed.is_empty() || new_leaf.is_some() {
        asset_parents(world, &candidate, source_root)?;
    }
    rebuild_globals(&mut candidate.entities, None, &[], Some(source_root))?;
    Ok(PrefabApplyPlan {
        asset: candidate,
        removed,
        new_leaf,
    })
}

fn validate_removals(
    world: &World,
    parents: &BTreeMap<EntityId, Option<EntityId>>,
    removed: &BTreeSet<EntityId>,
    earlier: &BTreeMap<EntityId, EntityId>,
) -> Result<(), String> {
    if parents.iter().any(|(id, parent)| {
        !removed.contains(id) && parent.is_some_and(|parent| removed.contains(&parent))
    }) {
        return Err("结构性 Apply 删除的来源分支仍有存活子节点".into());
    }
    for source in removed {
        if earlier
            .get(source)
            .is_none_or(|id| world.key(*id).is_some())
        {
            return Err(format!("来源实体 {source} 需要从已保存实例中删除"));
        }
    }
    Ok(())
}

fn project_leaf(
    world: &World,
    prefab: &WorldData,
    instance: &PrefabInstance,
    parents: &BTreeMap<EntityId, Option<EntityId>>,
    source_ids: &BTreeMap<EntityId, EntityId>,
    id: EntityId,
) -> Result<(NewLeaf, EntityData), String> {
    let mut row = world
        .entity_data(id)
        .ok_or_else(|| format!("无法读取新增实例实体：{id}"))?;
    if row
        .components
        .iter()
        .any(|component| component.component == PrefabInstance::ID)
    {
        return Err(format!("新增实体 {id} 已有关联的预制体来源"));
    }
    let mut has_children = false;
    world.for_each::<Parent>(|_, parent| has_children |= parent.entity == id);
    if has_children {
        return Err(format!("新增实体 {id} 不是叶节点"));
    }
    if parents.contains_key(&id) {
        return Err(format!("草稿实体 {id} 与现有资产实体 ID 冲突"));
    }
    // 当前实例化仅重写 Parent，因此不能把其他字段里的场景 ID 带进资产。
    if row
        .components
        .iter()
        .filter(|component| component.component != Parent::ID)
        .flat_map(|component| &component.fields)
        .any(|(_, value)| contains_entity_reference(value))
    {
        return Err(format!("新增实体 {id} 含有无法映射的场景实体引用"));
    }
    remap_parent(&mut row, source_ids);
    row.revision = prefab.revision;
    let last = parents.last_key_value().expect("资产根已校验").0.raw();
    let minimum = EntityId::from_raw(
        prefab
            .next_id
            .max(world.next_entity_id().raw())
            .max(last.max(id.raw()) + 1),
    );
    minimum
        .validate()
        .map_err(|err| format!("新增预制体叶节点无法领号：{err}"))?;
    Ok((
        NewLeaf {
            local: id,
            source: instance.source.clone(),
            minimum,
        },
        row,
    ))
}

fn contains_entity_reference(value: &Value) -> bool {
    match value {
        Value::Entity(_) => true,
        Value::List(values) => values.iter().any(contains_entity_reference),
        Value::Struct(values) | Value::Map(values) => {
            values.values().any(contains_entity_reference)
        }
        Value::Enum { payload, .. } => contains_entity_reference(payload),
        _ => false,
    }
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
