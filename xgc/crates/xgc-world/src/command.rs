//! M1 命令事务：plan → apply → revert。
//!
//! 已支持三类命令：字段写入、按实体数据生成、销毁。`Spawn` / `Despawn` 的
//! before-image 是 [`EntityData`]（可序列化）；回滚按逆命令**逆序**执行，
//! 交易只推进一个 revision。

use serde::{Deserialize, Serialize};
use xgc_base::diagnostics as codes;
use xgc_base::{Diagnostic, EntityId, Severity, Subject};
use xgc_reflect::{ComponentId, Value};

use super::{ComponentData, EntityData, World};

mod plan;
mod replace;

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
    Replace {
        data: EntityData,
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
                Command::Replace { data } => {
                    let Some(original) = self.entity_data(data.id) else {
                        return Err(vec![diag(
                            codes::EDIT_UNKNOWN_ENTITY,
                            Subject::Entity(data.id),
                            format!("替换目标不存在：{}", data.id),
                        )]);
                    };
                    self.replace_entity_raw(&original, data).map(|_| {
                        inverse.push(Command::Replace { data: original });
                        touched.push(data.id);
                    })
                }
                Command::Despawn { entity } => {
                    let (Some(key), Some(data)) = (self.key_of(*entity), self.entity_data(*entity))
                    else {
                        return Err(vec![diag(
                            codes::EDIT_CAPTURE_FAILED,
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
                    codes::EDIT_APPLY_FAILED,
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
                Command::Replace { data } => {
                    let Some(original) = self.entity_data(data.id) else {
                        return Err(vec![diag(
                            codes::EDIT_UNKNOWN_ENTITY,
                            Subject::Entity(data.id),
                            format!("回滚时替换目标不存在：{}", data.id),
                        )]);
                    };
                    self.replace_entity_raw(&original, data).map(|_| {
                        touched.push(data.id);
                    })
                }
                Command::Despawn { entity } => {
                    let Some(key) = self.key_of(*entity) else {
                        return Err(vec![diag(
                            codes::EDIT_UNKNOWN_ENTITY,
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
                    codes::EDIT_REVERT_FAILED,
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
        codes::EDIT_REVISION_CONFLICT,
        Subject::System("edit".to_string()),
        format!("计划基于旧 revision；当前已经是 {current}"),
    )
}
