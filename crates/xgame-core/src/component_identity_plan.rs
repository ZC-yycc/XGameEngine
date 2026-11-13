//! 从运行时值模型规划组件身份：只计算映射，不改实体或写文件。
//!
//! [`Entity`] 本身不持有组件实例 ID；新建文件和运行时生成实体都通过这里
//! 按确定顺序分配身份。与来源预制体对位时只认无歧义的种类、注册名或脚本路径。

use std::collections::{BTreeMap, BTreeSet};

use crate::component_id::{ComponentIdSequence, ComponentIdentity};
use crate::{BuiltinComponent, ComponentAddress, Entity};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedComponentIdentity {
    pub address: ComponentAddress,
    pub name: String,
    pub identity: ComponentIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeComponentIdentityPlan {
    pub next_component_id: ComponentIdSequence,
    pub components: Vec<PlannedComponentIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ComponentKey {
    Builtin(BuiltinComponent),
    Rust(String),
    Lua(String),
}

fn key(address: ComponentAddress, name: &str) -> ComponentKey {
    match address {
        ComponentAddress::Builtin(kind) => ComponentKey::Builtin(kind),
        ComponentAddress::Rust(_) => ComponentKey::Rust(name.to_owned()),
        ComponentAddress::Lua(_) => ComponentKey::Lua(name.to_owned()),
    }
}

/// 规划每个现有挂载物的本地 ID；`source` 是 `PrefabInstanceRef.source_id` 对应的资产实体。
///
/// 相同 Rust 名共享参数，同脚本路径共享状态：运行时值模型无法区分多个实例，
/// 因此拒绝分配歧义身份。来源未匹配的本地条目视为实例自行添加，不伪造来源 ID。
pub fn plan_runtime_component_identities(
    entity: &Entity,
    source: Option<&Entity>,
) -> Result<RuntimeComponentIdentityPlan, String> {
    let source_ids = if let Some(source) = source {
        let source_plan = plan_runtime_component_identities(source, None)?;
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
                "实体 {:?} 中 {:?} 重复，运行时值模型无法确定组件来源",
                entity.name, component.name
            ));
        }
        let id = next.allocate()?;
        let source_component_id = source_ids.get(&identity_key).copied();
        components.push(PlannedComponentIdentity {
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
                "实体 {:?} 的 Rust 参数 {:?} 没有对应的行为，投影会丢数据",
                entity.name, name
            ));
        }
    }
    Ok(RuntimeComponentIdentityPlan {
        next_component_id: next,
        components,
    })
}
