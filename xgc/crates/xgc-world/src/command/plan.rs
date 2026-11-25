//! 只读规划。在需要顺序语义时，以受影响实体的快照推演后续命令。

use std::collections::{HashMap, HashSet};

use xgc_base::diagnostics as codes;
use xgc_base::{Diagnostic, EntityId, Subject};

use super::{diag, Command, ComponentId, Plan, Value, World};
use crate::field_edit::{is_nested, FieldEditError};
use crate::EntitySnapshot;

struct Planner<'a> {
    world: &'a World,
    staged: HashMap<EntityId, EntitySnapshot>,
    dead: HashSet<EntityId>,
    path_targets: HashSet<EntityId>,
}

impl World {
    pub fn plan(&self, commands: Vec<Command>) -> Plan {
        let mut planner = Planner {
            world: self,
            staged: HashMap::new(),
            dead: HashSet::new(),
            path_targets: HashSet::new(),
        };
        if commands.len() > 1 {
            // 同一计划可先增删列表项目，再写其中一项。只为用到路径的实体预演写入，
            // 普通脚本标量命令仍然直接在当前组件上做只读校验。
            planner
                .path_targets
                .extend(commands.iter().filter_map(|command| match command {
                    Command::SetField { entity, field, .. } if is_nested(field) => Some(*entity),
                    _ => None,
                }));
        }
        let mut diagnostics = Vec::new();
        let mut accepted = Vec::with_capacity(commands.len());
        for command in commands {
            match planner.check(&command) {
                Ok(()) => accepted.push(command),
                Err(error) => diagnostics.push(error),
            }
        }
        Plan {
            base_revision: self.revision(),
            commands: accepted,
            diagnostics,
        }
    }
}

impl Planner<'_> {
    fn check(&mut self, command: &Command) -> Result<(), Diagnostic> {
        match command {
            Command::SetField {
                entity,
                component,
                field,
                value,
            } => self.check_field(*entity, *component, field, value),
            Command::Spawn { data } => {
                let id = data.id;
                if self.exists(id) || self.dead.contains(&id) {
                    return Err(diag(
                        codes::EDIT_ID_IN_USE,
                        Subject::Entity(id),
                        format!("实体 ID 已被占用或在同一计划里重复：{id}"),
                    ));
                }
                let snapshot = self.world.snapshot_from_data(data).map_err(|message| {
                    diag(codes::EDIT_INVALID_SPAWN, Subject::Entity(id), message)
                })?;
                self.staged.insert(id, snapshot);
                Ok(())
            }
            Command::AddComponent { entity, component } => {
                let source = self.world;
                let snapshot = self.stage(*entity)?;
                if snapshot
                    .components
                    .iter()
                    .any(|(id, _)| *id == component.component)
                {
                    return Err(diag(
                        codes::EDIT_COMPONENT_EXISTS,
                        Subject::Entity(*entity),
                        format!("实体 {entity} 已有组件 {}", component.component),
                    ));
                }
                let probe = super::EntityData {
                    id: *entity,
                    revision: 0,
                    components: vec![component.clone()],
                };
                let added = source.snapshot_from_data(&probe).map_err(|message| {
                    diag(
                        codes::EDIT_INVALID_COMPONENT,
                        Subject::Entity(*entity),
                        message,
                    )
                })?;
                snapshot.components.extend(added.components);
                Ok(())
            }
            Command::RemoveComponent { entity, component } => {
                let snapshot = self.stage(*entity)?;
                let index = snapshot
                    .components
                    .iter()
                    .position(|(id, _)| id == component)
                    .ok_or_else(|| {
                        diag(
                            codes::EDIT_COMPONENT_MISSING,
                            Subject::Entity(*entity),
                            format!("实体 {entity} 没有组件 {component}"),
                        )
                    })?;
                snapshot.components.remove(index);
                Ok(())
            }
            Command::Replace { data } => {
                let id = data.id;
                if self.dead.contains(&id) || !self.world.by_id.contains_key(&id) {
                    return Err(diag(
                        codes::EDIT_UNKNOWN_ENTITY,
                        Subject::Entity(id),
                        format!("替换目标不存在：{id}"),
                    ));
                }
                let snapshot = self.world.snapshot_from_data(data).map_err(|message| {
                    diag(codes::EDIT_INVALID_SPAWN, Subject::Entity(id), message)
                })?;
                self.staged.insert(id, snapshot);
                Ok(())
            }
            Command::Despawn { entity } => {
                self.require_alive(*entity)?;
                if !self.world.by_id.contains_key(entity) {
                    return Err(unknown_entity(*entity));
                }
                self.dead.insert(*entity);
                self.staged.remove(entity);
                Ok(())
            }
        }
    }

    fn exists(&self, id: EntityId) -> bool {
        !self.dead.contains(&id)
            && (self.staged.contains_key(&id) || self.world.by_id.contains_key(&id))
    }

    fn require_alive(&self, id: EntityId) -> Result<(), Diagnostic> {
        if self.dead.contains(&id) {
            return Err(diag(
                codes::EDIT_ENTITY_DEAD_IN_PLAN,
                Subject::Entity(id),
                format!("实体 {id} 在同一计划里已被销毁"),
            ));
        }
        if !self.exists(id) {
            return Err(unknown_entity(id));
        }
        Ok(())
    }

    fn stage(&mut self, id: EntityId) -> Result<&mut EntitySnapshot, Diagnostic> {
        self.require_alive(id)?;
        match self.staged.entry(id) {
            std::collections::hash_map::Entry::Occupied(entry) => Ok(entry.into_mut()),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let snapshot = self
                    .world
                    .capture_entity(id)
                    .ok_or_else(|| unknown_entity(id))?;
                Ok(entry.insert(snapshot))
            }
        }
    }

    fn check_field(
        &mut self,
        id: EntityId,
        component: ComponentId,
        field: &str,
        value: &Value,
    ) -> Result<(), Diagnostic> {
        let missing = || {
            diag(
                codes::EDIT_UNKNOWN_COMPONENT,
                Subject::Entity(id),
                format!("实体 {id} 没有组件 {component}"),
            )
        };
        let invalid = |error| {
            let code = match error {
                FieldEditError::Path(_) => codes::EDIT_UNKNOWN_FIELD,
                FieldEditError::Value(_) => codes::EDIT_TYPE_MISMATCH,
            };
            diag(
                code,
                Subject::Field(format!("{component}.{field}")),
                error.message(),
            )
        };
        if self.staged.contains_key(&id) || self.path_targets.contains(&id) || is_nested(field) {
            let (_, column) = self
                .stage(id)?
                .components
                .iter_mut()
                .find(|(id, _)| *id == component)
                .ok_or_else(missing)?;
            return column
                .write_field(0, field, value)
                .map(|_| ())
                .map_err(invalid);
        }

        self.require_alive(id)?;
        let (_, meta) = self
            .world
            .meta_by_id(id)
            .ok_or_else(|| unknown_entity(id))?;
        let archetype = self
            .world
            .archetypes
            .get(meta.archetype as usize)
            .ok_or_else(|| {
                diag(
                    codes::EDIT_UNKNOWN_ARCHETYPE,
                    Subject::Entity(id),
                    "实体所在的 archetype 不存在",
                )
            })?;
        let column = archetype.columns.get(&component).ok_or_else(missing)?;
        let row = meta.row as usize;
        if column.get_field(row, field).is_none() {
            return Err(invalid(FieldEditError::Path(format!(
                "组件 {component} 没有字段 {field}"
            ))));
        }
        column
            .validate_field(row, field, value)
            .map_err(|message| invalid(FieldEditError::Value(message)))
    }
}

fn unknown_entity(id: EntityId) -> Diagnostic {
    diag(
        codes::EDIT_UNKNOWN_ENTITY,
        Subject::Entity(id),
        format!("实体不存在：{id}"),
    )
}
