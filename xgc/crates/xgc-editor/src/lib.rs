//! 编辑器无头状态模型：场景打开 / 保存、实体选择、Inspector 字段、Play 快照。
//!
//! UI 层只读这里的状态并发编辑命令；这样编辑器逻辑可以脱离窗口测试。

mod app;
pub use app::run;

use std::path::{Path, PathBuf};

use xgc_base::EntityId;
use xgc_reflect::{Registry, Value};
use xgc_sim::{FrameRecord, Simulation};
use xgc_store::{load_world, save_world};
use xgc_world::{
    builtin_registry, Command, ComponentData, ComponentId, Name, Transform, World, WorldData,
};

pub struct EntityRow {
    pub id: EntityId,
    pub name: String,
    pub parent: Option<EntityId>,
}

pub struct FieldRow {
    pub name: String,
    pub ty: String,
    pub value: Value,
}

pub struct ComponentRow {
    pub id: u32,
    pub name: String,
    pub fields: Vec<FieldRow>,
}

struct PlaySession {
    before: WorldData,
    simulation: Simulation,
}

pub struct EditorState {
    world: World,
    registry: Registry,
    path: Option<PathBuf>,
    selected: Option<EntityId>,
    dirty: bool,
    play: Option<PlaySession>,
    undo: Vec<WorldData>,
    redo: Vec<WorldData>,
}

impl Default for EditorState {
    fn default() -> Self {
        Self::new(World::new())
    }
}

impl EditorState {
    pub fn new(world: World) -> Self {
        Self {
            world,
            registry: builtin_registry(),
            path: None,
            selected: None,
            dirty: false,
            play: None,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let data = load_world(path).map_err(|err| format!("读取世界失败：{err}"))?;
        let mut world = World::new();
        world
            .restore_world_data(&data)
            .map_err(|err| format!("恢复世界失败：{err}"))?;
        Ok(Self {
            world,
            registry: builtin_registry(),
            path: Some(path.to_path_buf()),
            selected: None,
            dirty: false,
            play: None,
            undo: Vec::new(),
            redo: Vec::new(),
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn is_playing(&self) -> bool {
        self.play.is_some()
    }

    pub fn world(&self) -> &World {
        match &self.play {
            Some(session) => session.simulation.world(),
            None => &self.world,
        }
    }

    /// Play 期间禁止直接编辑；Inspector 只读。
    pub fn world_mut(&mut self) -> Option<&mut World> {
        if self.play.is_some() {
            None
        } else {
            Some(&mut self.world)
        }
    }

    pub fn selected(&self) -> Option<EntityId> {
        self.selected
    }

    pub fn select(&mut self, id: Option<EntityId>) {
        self.selected = id;
    }

    pub fn component_names(&self) -> Vec<String> {
        self.registry
            .schema()
            .types
            .iter()
            .map(|ty| ty.name.clone())
            .collect()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn push_undo(&mut self) {
        let snapshot = self.world.world_data();
        if self.undo.last() == Some(&snapshot) {
            return;
        }
        self.undo.push(snapshot);
        if self.undo.len() > 64 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn restore_snapshot(&mut self, snapshot: WorldData) -> Result<(), String> {
        let mut world = World::new();
        world
            .restore_world_data(&snapshot)
            .map_err(|err| format!("恢复历史失败：{err}"))?;
        self.world = world;
        if self.selected.is_some_and(|id| self.world.key(id).is_none()) {
            self.selected = None;
        }
        self.dirty = true;
        Ok(())
    }

    pub fn undo(&mut self) -> Result<bool, String> {
        if self.play.is_some() {
            return Err("Play 期间不能撤销".to_string());
        }
        let Some(snapshot) = self.undo.pop() else {
            return Ok(false);
        };
        self.redo.push(self.world.world_data());
        self.restore_snapshot(snapshot)?;
        Ok(true)
    }

    pub fn redo(&mut self) -> Result<bool, String> {
        if self.play.is_some() {
            return Err("Play 期间不能重做".to_string());
        }
        let Some(snapshot) = self.redo.pop() else {
            return Ok(false);
        };
        self.undo.push(self.world.world_data());
        self.restore_snapshot(snapshot)?;
        Ok(true)
    }

    /// 新建实体（Name + Transform），返回持久 ID。
    pub fn spawn_entity(&mut self, name: &str) -> Result<EntityId, String> {
        if self.play.is_some() {
            return Err("Play 期间不能新建实体".to_string());
        }
        self.push_undo();
        let key = self.world.spawn((
            Name {
                value: name.to_string(),
            },
            Transform::default(),
        ));
        let id = self
            .world
            .entity(key)
            .ok_or_else(|| "新建实体失败".to_string())?;
        self.selected = Some(id);
        self.dirty = true;
        Ok(id)
    }

    pub fn delete_selected(&mut self) -> Result<(), String> {
        if self.play.is_some() {
            return Err("Play 期间不能删除实体".to_string());
        }
        let Some(id) = self.selected else {
            return Err("未选择实体".to_string());
        };
        let key = self
            .world
            .key(id)
            .ok_or_else(|| format!("实体 {} 不存在", id.raw()))?;
        self.push_undo();
        self.world.despawn(key);
        self.selected = None;
        self.dirty = true;
        Ok(())
    }

    pub fn add_component(&mut self, entity: EntityId, name: &str) -> Result<(), String> {
        if self.play.is_some() {
            return Err("Play 期间不能加组件".to_string());
        }
        self.push_undo();
        let schema = self
            .registry
            .by_name(name)
            .ok_or_else(|| format!("未知组件：{name}"))?;
        let component = ComponentData {
            component: ComponentId(schema.id),
            fields: schema
                .fields
                .iter()
                .map(|field| (field.name.clone(), field.default.clone()))
                .collect(),
        };
        self.apply_command(Command::AddComponent { entity, component })?;
        self.dirty = true;
        Ok(())
    }

    pub fn remove_component(&mut self, entity: EntityId, component: u32) -> Result<(), String> {
        if self.play.is_some() {
            return Err("Play 期间不能删组件".to_string());
        }
        self.push_undo();
        self.apply_command(Command::RemoveComponent {
            entity,
            component: ComponentId(component),
        })?;
        self.dirty = true;
        Ok(())
    }

    fn apply_command(&mut self, command: Command) -> Result<(), String> {
        let plan = self.world.plan(vec![command]);
        if !plan.is_valid() {
            return Err(format!("命令计划无效：{:?}", plan.diagnostics));
        }
        self.world
            .apply(&plan)
            .map_err(|diagnostics| format!("命令应用失败：{diagnostics:?}"))?;
        Ok(())
    }

    pub fn entities(&self) -> Vec<EntityRow> {
        let mut rows = Vec::new();
        let Some(data) = self.world_data_for_view() else {
            return rows;
        };
        for entity in &data.entities {
            let name = entity
                .components
                .iter()
                .find(|component| component.component.0 == 0x0103)
                .and_then(|component| {
                    component.fields.iter().find_map(|(field, value)| {
                        (field == "value").then(|| value_as_string(value))
                    })
                })
                .unwrap_or_else(|| format!("#{}", entity.id.raw()));
            let parent = entity
                .components
                .iter()
                .find(|component| component.component.0 == 0x0104)
                .and_then(|component| {
                    component.fields.iter().find_map(|(field, value)| {
                        (field == "entity").then(|| match value {
                            Value::Entity(raw) => Some(EntityId::from_raw(*raw)),
                            _ => None,
                        })?
                    })
                });
            rows.push(EntityRow {
                id: entity.id,
                name,
                parent,
            });
        }
        rows.sort_by_key(|row| row.id);
        rows
    }

    fn world_data_for_view(&self) -> Option<WorldData> {
        Some(match &self.play {
            Some(session) => session.simulation.world().world_data(),
            None => self.world.world_data(),
        })
    }

    pub fn inspector(&self) -> Vec<ComponentRow> {
        let Some(id) = self.selected else {
            return Vec::new();
        };
        let Some(data) = self.entity_data(id) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for component in &data.components {
            let Some(schema) = self.registry.get(component.component) else {
                continue;
            };
            let fields = schema
                .fields
                .iter()
                .map(|field| {
                    let value = component
                        .fields
                        .iter()
                        .find(|(name, _)| name == &field.name)
                        .map(|(_, value)| value.clone())
                        .unwrap_or_else(|| field.default.clone());
                    FieldRow {
                        name: field.name.clone(),
                        ty: field.ty.clone(),
                        value,
                    }
                })
                .collect();
            rows.push(ComponentRow {
                id: schema.id,
                name: schema.name.clone(),
                fields,
            });
        }
        rows.sort_by_key(|row| row.id);
        rows
    }

    fn entity_data(&self, id: EntityId) -> Option<xgc_world::EntityData> {
        match &self.play {
            Some(session) => session.simulation.world().entity_data(id),
            None => self.world.entity_data(id),
        }
    }

    pub fn edit_field(
        &mut self,
        entity: EntityId,
        component: u32,
        field: &str,
        value: Value,
    ) -> Result<(), String> {
        if self.play.is_some() {
            return Err("Play 期间不能编辑".to_string());
        }
        self.push_undo();
        self.world
            .set_field(entity, xgc_world::ComponentId(component), field, &value)?;
        self.dirty = true;
        Ok(())
    }

    pub fn save(&mut self) -> Result<(), String> {
        if self.play.is_some() {
            return Err("Play 期间不能保存".to_string());
        }
        let path = self
            .path
            .clone()
            .ok_or_else(|| "尚未指定场景路径".to_string())?;
        save_world(&path, &self.world.world_data()).map_err(|err| format!("保存失败：{err}"))?;
        self.dirty = false;
        Ok(())
    }

    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
        if self.play.is_some() {
            return Err("Play 期间不能保存".to_string());
        }
        let path = path.as_ref().to_path_buf();
        save_world(&path, &self.world.world_data()).map_err(|err| format!("保存失败：{err}"))?;
        self.path = Some(path);
        self.dirty = false;
        Ok(())
    }

    pub fn play(&mut self) -> Result<(), String> {
        if self.play.is_some() {
            return Ok(());
        }
        let before = self.world.world_data();
        let mut world = World::new();
        world
            .restore_world_data(&before)
            .map_err(|err| format!("进入 Play 失败：{err}"))?;
        self.play = Some(PlaySession {
            before,
            simulation: Simulation::new(world),
        });
        Ok(())
    }

    pub fn step(&mut self) -> Result<Option<FrameRecord>, String> {
        let Some(session) = &mut self.play else {
            return Ok(None);
        };
        session
            .simulation
            .step(Vec::new())
            .map(Some)
            .map_err(|err| format!("Play 帧失败：{err:?}"))
    }

    pub fn stop(&mut self) -> Result<(), String> {
        let Some(session) = self.play.take() else {
            return Ok(());
        };
        let mut world = World::new();
        world
            .restore_world_data(&session.before)
            .map_err(|err| format!("退出 Play 失败：{err}"))?;
        self.world = world;
        self.selected = None;
        self.dirty = false;
        Ok(())
    }
}

fn value_as_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        other => format!("{other:?}"),
    }
}
