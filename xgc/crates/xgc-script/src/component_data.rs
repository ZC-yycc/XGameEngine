//! 内建组件的默认值集合。
//!
//! `lua_component`（脚本里按名字给实体挂组件）与 `ui_spawn_api`（运行时建 UI 实体）
//! 都要一份「全字段 = schema 默认值」的 `ComponentData`。放在这里避免两边各抄一份
//! 字段清单——schema 加字段时只改这一处。

use xgc_world::{builtin_registry, ComponentData};

/// 按组件名生成全默认值的组件数据；名字未知时报 Lua 运行时错误。
pub(crate) fn default_component(name: &str) -> mlua::Result<ComponentData> {
    let registry = builtin_registry();
    let schema = registry
        .by_name(name)
        .ok_or_else(|| mlua::Error::RuntimeError(format!("未知组件：{name}")))?;
    Ok(ComponentData {
        component: xgc_world::ComponentId(schema.id),
        fields: schema
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.default.clone()))
            .collect(),
    })
}
