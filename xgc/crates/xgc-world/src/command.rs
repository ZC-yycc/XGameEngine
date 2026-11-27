//! M1 命令事务：plan → apply → revert。
//!
//! 结构变更的 before-image 是可序列化的 [`EntityData`]。撤销按逆命令逆序
//! 提交，和普通编辑一样只推进一次 revision。

use serde::{Deserialize, Serialize};
use xgc_base::diagnostics as codes;
use xgc_base::{Diagnostic, EntityId, Severity, Subject};
use xgc_reflect::{ComponentId, Value};

use super::{ComponentData, EntityData, Journal, World};

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
    /// 原子应用：先校验 revision；中途失败则按逆命令逆序回退。
    pub fn apply(&mut self, plan: &Plan) -> Result<Applied, Vec<Diagnostic>> {
        self.apply_transaction(plan, false, None)
    }

    /// 应用并记账；日志记在内存里，落盘由上层负责。
    pub fn apply_logged(
        &mut self,
        plan: &Plan,
        journal: &mut Journal,
    ) -> Result<Applied, Vec<Diagnostic>> {
        self.apply_transaction(plan, false, Some(journal))
    }

    /// 回滚一次已应用的交易；要求世界从应用之后没有再变过。
    pub fn revert(&mut self, applied: &Applied) -> Result<u64, Vec<Diagnostic>> {
        self.revert_transaction(applied, None)
    }

    /// 将撤销作为一笔新事务提交，使日志连续覆盖世界的每次编辑。
    pub fn revert_logged(
        &mut self,
        applied: &Applied,
        journal: &mut Journal,
    ) -> Result<u64, Vec<Diagnostic>> {
        self.revert_transaction(applied, Some(journal))
    }

    fn revert_transaction(
        &mut self,
        applied: &Applied,
        journal: Option<&mut Journal>,
    ) -> Result<u64, Vec<Diagnostic>> {
        let plan = Plan {
            base_revision: applied.revision,
            commands: applied.inverse.iter().rev().cloned().collect(),
            diagnostics: Vec::new(),
        };
        self.apply_transaction(&plan, true, journal)
            .map(|reverted| reverted.revision)
    }

    fn apply_transaction(
        &mut self,
        plan: &Plan,
        reverting: bool,
        journal: Option<&mut Journal>,
    ) -> Result<Applied, Vec<Diagnostic>> {
        if !plan.is_valid() {
            return Err(plan.diagnostics.clone());
        }
        if plan.base_revision != self.revision() {
            return Err(vec![conflict(self.revision())]);
        }
        // 公开的 Plan 可以绕过规划器。结构回滚会换代句柄，所以要先检查坏 ID。
        for command in &plan.commands {
            if let Command::Spawn { data } | Command::Replace { data } = command {
                validate_data_id(data.id).map_err(|error| vec![error])?;
            }
        }

        let next_id = self.next_id;
        let mut inverse = Vec::with_capacity(plan.commands.len());
        for command in &plan.commands {
            match self.execute_command(command, reverting) {
                Ok(command) => inverse.push(command),
                Err(error) => {
                    let mut errors = vec![error];
                    for command in inverse.iter().rev() {
                        if let Err(error) = self.execute_command(command, true) {
                            errors.push(error);
                        }
                    }
                    self.next_id = next_id;
                    return Err(errors);
                }
            }
        }

        let revision = if plan.commands.is_empty() {
            self.revision()
        } else {
            self.bump_revision()
        };
        for command in &plan.commands {
            let id = match command {
                Command::SetField { entity, .. }
                | Command::AddComponent { entity, .. }
                | Command::RemoveComponent { entity, .. } => *entity,
                Command::Spawn { data } | Command::Replace { data } => data.id,
                Command::Despawn { .. } => continue,
            };
            self.touch(id, revision);
        }
        let applied = Applied { revision, inverse };
        if let Some(journal) = journal {
            journal.record(plan, &applied);
        }
        Ok(applied)
    }

    fn execute_command(
        &mut self,
        command: &Command,
        reverting: bool,
    ) -> Result<Command, Diagnostic> {
        let inverse = match command {
            Command::SetField {
                entity,
                component,
                field,
                value,
            } => self
                .set_field_raw(*entity, *component, field, value)
                .map(|before| Command::SetField {
                    entity: *entity,
                    component: *component,
                    field: field.clone(),
                    value: before,
                }),
            Command::Spawn { data } => self
                .snapshot_from_data(data)
                .and_then(|snapshot| self.restore_entity_raw(snapshot))
                .map(|entity| Command::Despawn { entity }),
            Command::AddComponent { entity, component } => self
                .add_component_raw(*entity, component)
                .map(|()| Command::RemoveComponent {
                    entity: *entity,
                    component: component.component,
                }),
            Command::RemoveComponent { entity, component } => self
                .remove_component_raw(*entity, *component)
                .map(|component| Command::AddComponent {
                    entity: *entity,
                    component,
                }),
            Command::Replace { data } => {
                let original = self.entity_data(data.id).ok_or_else(|| {
                    let prefix = if reverting { "回滚时" } else { "" };
                    diag(
                        codes::EDIT_UNKNOWN_ENTITY,
                        Subject::Entity(data.id),
                        format!("{prefix}替换目标不存在：{}", data.id),
                    )
                })?;
                self.replace_entity_raw(&original, data)
                    .map(|()| Command::Replace { data: original })
            }
            Command::Despawn { entity } => {
                let capture_failed = || {
                    diag(
                        codes::EDIT_CAPTURE_FAILED,
                        Subject::Entity(*entity),
                        format!("实体快照失败：{entity}"),
                    )
                };
                let key = self.key_of(*entity).ok_or_else(|| {
                    if reverting {
                        diag(
                            codes::EDIT_UNKNOWN_ENTITY,
                            Subject::Entity(*entity),
                            format!("回滚时实体不存在：{entity}"),
                        )
                    } else {
                        capture_failed()
                    }
                })?;
                let data = self.entity_data(*entity).ok_or_else(capture_failed)?;
                if self.despawn_raw(key) {
                    Ok(Command::Spawn { data })
                } else {
                    let prefix = if reverting { "回滚 " } else { "" };
                    Err(format!("{prefix}despawn 失败"))
                }
            }
        };
        inverse.map_err(|message| {
            let code = if reverting {
                codes::EDIT_REVERT_FAILED
            } else {
                codes::EDIT_APPLY_FAILED
            };
            diag(code, Subject::System("edit".to_string()), message)
        })
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

fn validate_data_id(id: EntityId) -> Result<(), Diagnostic> {
    id.validate().map_err(|error| {
        diag(
            codes::EDIT_INVALID_SPAWN,
            Subject::Entity(id),
            error.to_string(),
        )
    })
}

fn conflict(current: u64) -> Diagnostic {
    diag(
        codes::EDIT_REVISION_CONFLICT,
        Subject::System("edit".to_string()),
        format!("计划基于旧 revision；当前已经是 {current}"),
    )
}
