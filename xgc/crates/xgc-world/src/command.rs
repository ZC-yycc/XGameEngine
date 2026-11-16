//! M1 最小命令事务：plan → apply → revert。
//!
//! 这一版只支持**单字段写入**（结构不变）。命令是数据、计划带 before-image、
//! 应用后 revision +1；`revert` 用逆命令回到旧值。结构化 spawn / despawn 在
//! 下一刀接上，接口保持同一条 `Command` 管线。

use xgc_base::{Diagnostic, EntityId, Severity, Subject};
use xgc_reflect::{ComponentId, Value};

use super::World;

#[derive(Clone, Debug)]
pub enum Command {
    SetField {
        entity: EntityId,
        component: ComponentId,
        field: String,
        value: Value,
    },
}

#[derive(Clone, Debug)]
pub struct PlannedOp {
    pub command: Command,
    /// 计划时的预览值；应用时仍以实际写入返回的 before 为准。
    pub before: Value,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub base_revision: u64,
    pub ops: Vec<PlannedOp>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Plan {
    pub fn is_valid(&self) -> bool {
        self.diagnostics.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct Applied {
    pub revision: u64,
    pub inverse: Vec<Command>,
}

impl World {
    /// 计划阶段只读：校验实体 / 组件 / 字段 / 类型，并记录 before-image。
    pub fn plan(&self, commands: Vec<Command>) -> Plan {
        let mut ops = Vec::with_capacity(commands.len());
        let mut diagnostics = Vec::new();

        for command in commands {
            let Command::SetField {
                entity,
                component,
                field,
                value,
            } = &command;

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
            let Some(before) = column.get_field(row, field) else {
                diagnostics.push(diag(
                    "EDIT.UNKNOWN_FIELD",
                    Subject::Field(format!("{component}.{field}")),
                    format!("组件 {component} 没有字段 {field}"),
                ));
                continue;
            };
            if let Err(message) = column.validate_field(row, field, value) {
                diagnostics.push(diag(
                    "EDIT.TYPE_MISMATCH",
                    Subject::Field(format!("{component}.{field}")),
                    message,
                ));
                continue;
            }

            ops.push(PlannedOp {
                command: command.clone(),
                before,
            });
        }

        Plan {
            base_revision: self.revision(),
            ops,
            diagnostics,
        }
    }

    /// 原子应用：先校验 revision；中途失败则按逆命令回滚。
    pub fn apply(&mut self, plan: &Plan) -> Result<Applied, Vec<Diagnostic>> {
        if !plan.is_valid() {
            return Err(plan.diagnostics.clone());
        }
        if plan.base_revision != self.revision() {
            return Err(vec![conflict(self.revision())]);
        }
        if plan.ops.is_empty() {
            return Ok(Applied {
                revision: self.revision(),
                inverse: Vec::new(),
            });
        }

        let mut inverse = Vec::with_capacity(plan.ops.len());
        let mut touched = Vec::with_capacity(plan.ops.len());
        for op in &plan.ops {
            let Command::SetField {
                entity,
                component,
                field,
                value,
            } = &op.command;

            match self.set_field_raw(*entity, *component, field, value) {
                Ok(before) => {
                    inverse.push(Command::SetField {
                        entity: *entity,
                        component: *component,
                        field: field.clone(),
                        value: before,
                    });
                    touched.push(*entity);
                }
                Err(message) => {
                    for undo in inverse.iter().rev() {
                        let Command::SetField {
                            entity,
                            component,
                            field,
                            value,
                        } = undo;
                        let _ = self.set_field_raw(*entity, *component, field, value);
                    }
                    return Err(vec![diag(
                        "EDIT.APPLY_FAILED",
                        Subject::Entity(*entity),
                        message,
                    )]);
                }
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

        let mut touched = Vec::with_capacity(applied.inverse.len());
        for command in applied.inverse.iter().rev() {
            let Command::SetField {
                entity,
                component,
                field,
                value,
            } = command;
            if let Err(message) = self.set_field_raw(*entity, *component, field, value) {
                return Err(vec![diag(
                    "EDIT.REVERT_FAILED",
                    Subject::Entity(*entity),
                    message,
                )]);
            }
            touched.push(*entity);
        }

        let revision = self.bump_revision();
        for id in touched {
            self.touch(id, revision);
        }
        Ok(revision)
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
