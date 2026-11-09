//! v1 → v2 的只读身份预览：仅计算映射，不改实体或写文件。
//!
//! 旧格式没有实例身份，不能把这里的数字当成已经持久化的组件 ID。
//! 与来源预制体对位时只认无歧义的种类、注册名或脚本路径。

use std::collections::{BTreeMap, BTreeSet};

use crate::component_id::{ComponentIdSequence, ComponentIdentity};
use crate::{BuiltinComponent, ComponentAddress, Entity};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedComponent {
    pub address: ComponentAddress,
    pub name: String,
    pub identity: ComponentIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentUpgradePlan {
    pub next_component_id: ComponentIdSequence,
    pub components: Vec<PlannedComponent>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum LegacyKey {
    Builtin(BuiltinComponent),
    Rust(String),
    Lua(String),
}

fn key(address: ComponentAddress, name: &str) -> LegacyKey {
    match address {
        ComponentAddress::Builtin(kind) => LegacyKey::Builtin(kind),
        ComponentAddress::Rust(_) => LegacyKey::Rust(name.to_owned()),
        ComponentAddress::Lua(_) => LegacyKey::Lua(name.to_owned()),
    }
}

/// 预览每个现有挂载物的本地 ID；`source` 是 `PrefabInstanceRef.source_id` 对应的资产实体。
///
/// 相同 Rust 名共享旧参数，同脚本路径共享旧状态：无法证明拆成多个实例无行为变化，
/// 因此拒绝自动迁移。来源未匹配的本地条目视为实例自行添加，不伪造来源 ID。
pub fn preview_component_upgrade(
    entity: &Entity,
    source: Option<&Entity>,
) -> Result<ComponentUpgradePlan, String> {
    let source_ids = if let Some(source) = source {
        let source_plan = preview_component_upgrade(source, None)?;
        let mut ids = BTreeMap::new();
        for component in source_plan.components {
            ids.insert(
                key(component.address, &component.name),
                component.identity.id,
            );
        }
        ids
    } else {
        BTreeMap::new()
    };
    let mut seen = BTreeSet::new();
    let mut next = ComponentIdSequence::default();
    let mut components = Vec::new();
    for component in entity.attached_components() {
        let identity_key = key(component.address, component.name);
        if !seen.insert(identity_key.clone()) {
            return Err(format!(
                "实体 {:?} 中 {:?} 重复，旧格式无法无损确定组件来源",
                entity.name, component.name
            ));
        }
        let id = next.allocate()?;
        let source_component_id = source_ids.get(&identity_key).copied();
        components.push(PlannedComponent {
            address: component.address,
            name: component.name.to_owned(),
            identity: ComponentIdentity {
                id,
                source_component_id,
            },
        });
    }
    for name in entity.behavior_params.keys() {
        if !entity.behaviors.contains(name) {
            return Err(format!(
                "实体 {:?} 的 Rust 参数 {:?} 没有对应的行为，自动升级会丢数据",
                entity.name, name
            ));
        }
    }
    Ok(ComponentUpgradePlan {
        next_component_id: next,
        components,
    })
}
