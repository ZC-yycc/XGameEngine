//! M1 命令事务：plan → apply → revert。
//!
//! 已支持三类命令：字段写入、按实体数据生成、销毁。`Spawn` / `Despawn` 的
//! before-image 是 [`EntityData`]（可序列化）；回滚按逆命令**逆序**执行，
//! 交易只推进一个 revision。

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use xgc_base::{Diagnostic, EntityId, Severity, Subject};
use xgc_reflect::{ComponentId, Value};

use super::{ComponentData, EntityData, World};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    SetField {
        entity: EntityId,
        component: ComponentId,
        field: String,
        value: Value,
    },
    AddComponent {
        entity: EntityId,
        component: ComponentData,
    },
    RemoveComponent {
        entity: EntityId,
        component: ComponentId,
    },
    Spawn {
        data: EntityData,
    },
    Despawn {
        entity: EntityId,
    },
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub base_revision: u64,
    pub commands: Vec<Command>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Plan {
    pub fn is_valid(&self) -> bool {
        self.diagnostics.is_empty()
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct Applied {
    pub revision: u64,
    pub inverse: Vec<Command>,
}

impl World {
    pub fn plan(&self, commands: Vec<Command>) -> Plan {
        let mut accepted = Vec::with_capacity(commands.len());
        let mut diagnostics = Vec::new();
        let mut new_ids = HashSet::new();
        let mut dead = HashSet::new();
        let mut presence: HashMap<EntityId, HashSet<ComponentId>> = HashMap::new();

        for command in commands {
            let mut valid = true;
            match &command {
                Command::SetField {
                    entity,
                    component,
                    field,
                    value,
                } => {
                    if dead.contains(entity) {
                        diagnostics.push(diag(
                            "EDIT.ENTITY_DEAD_IN_PLAN",
                            Subject::Entity(*entity),
                            format!("实体 {entity} 在同一计划里已被销毁"),
                        ));
                        continue;
                    }
                    if let Some(components) = presence.get(entity) {
                        if !components.contains(component) {
                            diagnostics.push(diag(
                                "EDIT.UNKNOWN_COMPONENT",
                                Subject::Entity(*entity),
                                format!("实体 {entity} 没有组件 {component}"),
                            ));
                            continue;
                        }
                        let Some(factory) = self.factories.get(component) else {
                            diagnostics.push(diag(
                                "EDIT.UNKNOWN_COMPONENT",
                                Subject::Entity(*entity),
                                format!("组件 {component} 未注册"),
                            ));
                            continue;
                        };
                        if let Err(message) = factory.validate_field(field, value) {
                            diagnostics.push(diag(
                                "EDIT.TYPE_MISMATCH",
                                Subject::Field(format!("{component}.{field}")),
                                message,
                            ));
                            valid = false;
                        }
                    } else {
                        let Some((_, meta)) = self.meta_by_id(*entity) else {
                            diagnostics.push(diag(
                                "EDIT.UNKNOWN_ENTITY",
                                Subject::Entity(*entity),
                                format!("实体不存在：{entity}"),
                            ));
                            continue;
                        };
                        let Some(arch) = self.archetypes.get(meta.archetype as usize) else {
                            diagnostics.push(diag(
                                "EDIT.UNKNOWN_ARCHETYPE",
                                Subject::Entity(*entity),
                                "实体所在的 archetype 不存在",
                            ));
                            continue;
                        };
                        let Some(column) = arch.columns.get(component) else {
                            diagnostics.push(diag(
                                "EDIT.UNKNOWN_COMPONENT",
                                Subject::Entity(*entity),
                                format!("实体 {entity} 没有组件 {component}"),
                            ));
                            continue;
                        };
                        let row = meta.row as usize;
                        if column.get_field(row, field).is_none() {
                            diagnostics.push(diag(
                                "EDIT.UNKNOWN_FIELD",
                                Subject::Field(format!("{component}.{field}")),
                                format!("组件 {component} 没有字段 {field}"),
                            ));
                            valid = false;
                        } else if let Err(message) = column.validate_field(row, field, value) {
                            diagnostics.push(diag(
                                "EDIT.TYPE_MISMATCH",
                                Subject::Field(format!("{component}.{field}")),
                                message,
                            ));
                            valid = false;
                        }
                    }
                }
                Command::Spawn { data } => {
                    let id = data.id;
                    if self.by_id.contains_key(&id) || new_ids.contains(&id) || dead.contains(&id) {
                        diagnostics.push(diag(
                            "EDIT.ID_IN_USE",
                            Subject::Entity(id),
                            format!("实体 ID 已被占用或在同一计划里重复：{id}"),
                        ));
                        valid = false;
                    } else if let Err(message) = self.snapshot_from_data(data) {
                        diagnostics.push(diag("EDIT.INVALID_SPAWN", Subject::Entity(id), message));
                        valid = false;
                    } else {
                        new_ids.insert(id);
                        presence.insert(
                            id,
                            data.components
                                .iter()
                                .map(|component| component.component)
                                .collect(),
                        );
                    }
                }
                Command::AddComponent { entity, component } => {
                    if dead.contains(entity) {
                        diagnostics.push(diag(
                            "EDIT.ENTITY_DEAD_IN_PLAN",
                            Subject::Entity(*entity),
                            format!("实体 {entity} 在同一计划里已被销毁"),
                        ));
                        continue;
                    }
                    if !new_ids.contains(entity) && self.meta_by_id(*entity).is_none() {
                        diagnostics.push(diag(
                            "EDIT.UNKNOWN_ENTITY",
                            Subject::Entity(*entity),
                            format!("实体不存在：{entity}"),
                        ));
                        continue;
                    }
                    let components = presence.entry(*entity).or_insert_with(|| {
                        self.entity_data(*entity)
                            .map(|data| {
                                data.components
                                    .into_iter()
                                    .map(|component| component.component)
                                    .collect()
                            })
                            .unwrap_or_default()
                    });
                    if components.contains(&component.component) {
                        diagnostics.push(diag(
                            "EDIT.COMPONENT_EXISTS",
                            Subject::Entity(*entity),
                            format!("实体 {entity} 已有组件 {}", component.component),
                        ));
                        valid = false;
                    } else {
                        let probe = EntityData {
                            id: *entity,
                            revision: 0,
                            components: vec![component.clone()],
                        };
                        if let Err(message) = self.snapshot_from_data(&probe) {
                            diagnostics.push(diag(
                                "EDIT.INVALID_COMPONENT",
                                Subject::Entity(*entity),
                                message,
                            ));
                            valid = false;
                        } else {
                            components.insert(component.component);
                        }
                    }
                }
                Command::RemoveComponent { entity, component } => {
                    if dead.contains(entity) {
                        diagnostics.push(diag(
                            "EDIT.ENTITY_DEAD_IN_PLAN",
                            Subject::Entity(*entity),
                            format!("实体 {entity} 在同一计划里已被销毁"),
                        ));
                        continue;
                    }
                    if !new_ids.contains(entity) && self.meta_by_id(*entity).is_none() {
                        diagnostics.push(diag(
                            "EDIT.UNKNOWN_ENTITY",
                            Subject::Entity(*entity),
                            format!("实体不存在：{entity}"),
                        ));
                        continue;
                    }
                    let components = presence.entry(*entity).or_insert_with(|| {
                        self.entity_data(*entity)
                            .map(|data| {
                                data.components
                                    .into_iter()
                                    .map(|component| component.component)
                                    .collect()
                            })
                            .unwrap_or_default()
                    });
                    if !components.remove(component) {
                        diagnostics.push(diag(
                            "EDIT.COMPONENT_MISSING",
                            Subject::Entity(*entity),
                            format!("实体 {entity} 没有组件 {component}"),
                        ));
                        valid = false;
                    }
                }
                Command::Despawn { entity } => {
                    if dead.contains(entity) {
                        diagnostics.push(diag(
                            "EDIT.ENTITY_DEAD_IN_PLAN",
                            Subject::Entity(*entity),
                            format!("实体 {entity} 在同一计划里已被销毁"),
                        ));
                        valid = false;
                    } else if self.meta_by_id(*entity).is_none() {
                        diagnostics.push(diag(
                            "EDIT.UNKNOWN_ENTITY",
                            Subject::Entity(*entity),
                            format!("实体不存在：{entity}"),
                        ));
                        valid = false;
                    } else {
                        dead.insert(*entity);
                    }
                }
            }

            if valid {
                accepted.push(command);
            }
        }

        Plan {
            base_revision: self.revision(),
            commands: accepted,
            diagnostics,
        }
    }

    /// 原子应用：先校验 revision；中途失败则按逆命令逆序回滚。
    pub fn apply(&mut self, plan: &Plan) -> Result<Applied, Vec<Diagnostic>> {
        if !plan.is_valid() {
            return Err(plan.diagnostics.clone());
        }
        if plan.base_revision != self.revision() {
            return Err(vec![conflict(self.revision())]);
        }
        if plan.commands.is_empty() {
            return Ok(Applied {
                revision: self.revision(),
                inverse: Vec::new(),
            });
        }

        let mut inverse = Vec::with_capacity(plan.commands.len());
        let mut touched = Vec::new();
        for command in &plan.commands {
            let result: Result<(), String> = match command {
                Command::SetField {
                    entity,
                    component,
                    field,
                    value,
                } => self
                    .set_field_raw(*entity, *component, field, value)
                    .map(|before| {
                        inverse.push(Command::SetField {
                            entity: *entity,
                            component: *component,
                            field: field.clone(),
                            value: before,
                        });
                        touched.push(*entity);
                    }),
                Command::Spawn { data } => match self.snapshot_from_data(data) {
                    Ok(snapshot) => self.restore_entity_raw(snapshot).map(|id| {
                        inverse.push(Command::Despawn { entity: id });
                        touched.push(id);
                    }),
                    Err(message) => Err(message),
                },
                Command::AddComponent { entity, component } => {
                    self.add_component_raw(*entity, component).map(|_| {
                        inverse.push(Command::RemoveComponent {
                            entity: *entity,
                            component: component.component,
                        });
                        touched.push(*entity);
                    })
                }
                Command::RemoveComponent { entity, component } => self
                    .remove_component_raw(*entity, *component)
                    .map(|removed| {
                        inverse.push(Command::AddComponent {
                            entity: *entity,
                            component: removed,
                        });
                        touched.push(*entity);
                    }),
                Command::Despawn { entity } => {
                    let (Some(key), Some(data)) = (self.key_of(*entity), self.entity_data(*entity))
                    else {
                        return Err(vec![diag(
                            "EDIT.CAPTURE_FAILED",
                            Subject::Entity(*entity),
                            format!("实体快照失败：{entity}"),
                        )]);
                    };
                    if self.despawn_raw(key) {
                        inverse.push(Command::Spawn { data });
                        Ok(())
                    } else {
                        Err("despawn 失败".to_string())
                    }
                }
            };

            if let Err(message) = result {
                let _ = self.rollback(&inverse);
                return Err(vec![diag(
                    "EDIT.APPLY_FAILED",
                    Subject::System("edit".to_string()),
                    message,
                )]);
            }
        }

        let revision = self.bump_revision();
        for id in touched {
            self.touch(id, revision);
        }
        Ok(Applied { revision, inverse })
    }

    /// 回滚一次已应用的交易；要求世界从应用之后没有再变过。
    pub fn revert(&mut self, applied: &Applied) -> Result<u64, Vec<Diagnostic>> {
        if applied.revision != self.revision() {
            return Err(vec![conflict(self.revision())]);
        }
        if applied.inverse.is_empty() {
            return Ok(self.revision());
        }

        let mut touched = Vec::new();
        for command in applied.inverse.iter().rev() {
            let result: Result<(), String> = match command {
                Command::SetField {
                    entity,
                    component,
                    field,
                    value,
                } => self
                    .set_field_raw(*entity, *component, field, value)
                    .map(|_| {
                        touched.push(*entity);
                    }),
                Command::Spawn { data } => match self.snapshot_from_data(data) {
                    Ok(snapshot) => self.restore_entity_raw(snapshot).map(|id| {
                        touched.push(id);
                    }),
                    Err(message) => Err(message),
                },
                Command::AddComponent { entity, component } => {
                    self.add_component_raw(*entity, component).map(|_| {
                        touched.push(*entity);
                    })
                }
                Command::RemoveComponent { entity, component } => {
                    self.remove_component_raw(*entity, *component).map(|_| {
                        touched.push(*entity);
                    })
                }
                Command::Despawn { entity } => {
                    let Some(key) = self.key_of(*entity) else {
                        return Err(vec![diag(
                            "EDIT.UNKNOWN_ENTITY",
                            Subject::Entity(*entity),
                            format!("回滚时实体不存在：{entity}"),
                        )]);
                    };
                    if self.despawn_raw(key) {
                        Ok(())
                    } else {
                        Err("回滚 despawn 失败".to_string())
                    }
                }
            };

            if let Err(message) = result {
                return Err(vec![diag(
                    "EDIT.REVERT_FAILED",
                    Subject::System("edit".to_string()),
                    message,
                )]);
            }
        }

        let revision = self.bump_revision();
        for id in touched {
            self.touch(id, revision);
        }
        Ok(revision)
    }

    fn rollback(&mut self, inverse: &[Command]) -> Result<(), String> {
        for command in inverse.iter().rev() {
            match command {
                Command::SetField {
                    entity,
                    component,
                    field,
                    value,
                } => {
                    self.set_field_raw(*entity, *component, field, value)?;
                }
                Command::Spawn { data } => {
                    let snapshot = self.snapshot_from_data(data)?;
                    self.restore_entity_raw(snapshot)?;
                }
                Command::AddComponent { entity, component } => {
                    self.add_component_raw(*entity, component)?;
                }
                Command::RemoveComponent { entity, component } => {
                    self.remove_component_raw(*entity, *component)?;
                }
                Command::Despawn { entity } => {
                    let Some(key) = self.key_of(*entity) else {
                        return Err(format!("回滚时实体不存在：{entity}"));
                    };
                    if !self.despawn_raw(key) {
                        return Err(format!("回滚 despawn 失败：{entity}"));
                    }
                }
            }
        }
        Ok(())
    }

    fn replace_entity_raw(
        &mut self,
        original: &EntityData,
        updated: &EntityData,
    ) -> Result<(), String> {
        let Some(key) = self.key_of(original.id) else {
            return Err(format!("实体不存在：{}", original.id));
        };
        let snapshot = self.snapshot_from_data(updated)?;
        if !self.despawn_raw(key) {
            return Err(format!("移除旧实体失败：{}", original.id));
        }
        if let Err(message) = self.restore_entity_raw(snapshot) {
            if let Ok(original_snapshot) = self.snapshot_from_data(original) {
                let _ = self.restore_entity_raw(original_snapshot);
            }
            return Err(message);
        }
        Ok(())
    }

    fn add_component_raw(
        &mut self,
        entity: EntityId,
        component: &ComponentData,
    ) -> Result<(), String> {
        let Some(original) = self.entity_data(entity) else {
            return Err(format!("实体不存在：{entity}"));
        };
        if original
            .components
            .iter()
            .any(|existing| existing.component == component.component)
        {
            return Err(format!("实体 {entity} 已有组件 {}", component.component));
        }
        let mut updated = original.clone();
        updated.components.push(component.clone());
        self.replace_entity_raw(&original, &updated)
    }

    fn remove_component_raw(
        &mut self,
        entity: EntityId,
        component: ComponentId,
    ) -> Result<ComponentData, String> {
        let Some(original) = self.entity_data(entity) else {
            return Err(format!("实体不存在：{entity}"));
        };
        let Some(position) = original
            .components
            .iter()
            .position(|existing| existing.component == component)
        else {
            return Err(format!("实体 {entity} 没有组件 {component}"));
        };
        let removed = original.components[position].clone();
        let mut updated = original.clone();
        updated.components.remove(position);
        self.replace_entity_raw(&original, &updated)?;
        Ok(removed)
    }
}

fn diag(code: &'static str, subject: Subject, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code, Severity::Error, subject, message)
}

fn conflict(current: u64) -> Diagnostic {
    diag(
        "EDIT.REVISION_CONFLICT",
        Subject::System("edit".to_string()),
        format!("计划基于旧 revision；当前已经是 {current}"),
    )
}
