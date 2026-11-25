//! 编辑器无头状态模型：场景打开 / 保存、实体选择、Inspector 字段、Play 快照。
//!
//! UI 层只读这里的状态并发编辑命令；这样编辑器逻辑可以脱离窗口测试。

mod app;
mod assets_ops;
mod gizmo;
mod play;
mod prefab_ops;
mod scene_ops;
mod transform_ops;
pub use app::run;
pub use assets_ops::{AssetEntry, AssetKind};
pub use transform_ops::snap;

use std::path::{Path, PathBuf};

use xgc_base::EntityId;
use xgc_reflect::{Registry, Value};
use xgc_script::ScriptRuntime;
use xgc_sim::{InputMap, ParticleSystem, Simulation};
use xgc_store::{load_world, save_world, ProjectSession};
use xgc_world::{
    builtin_registry, Camera, Command, ComponentData, ComponentId, GlobalTransform, Material,
    MeshRenderer, Name, RootMotionLibrary, Transform, World, WorldData,
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
    particles: ParticleSystem,
    runtime: Option<ScriptRuntime>,
    input: Option<InputMap>,
    /// 根运动轨道表：进入 Play 时从播放世界的模型资产建一次（与 CLI 宿主同源）。
    root_motion: RootMotionLibrary,
}

pub struct EditorState {
    world: World,
    registry: Registry,
    path: Option<PathBuf>,
    /// 从项目打开时才有；`path` 此时是入口场景路径，保存仍写场景本身。
    project: Option<ProjectSession>,
    /// 当前场景在 `project.xgc` 里的登记名；单场景打开时为 None。
    scene: Option<String>,
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
            project: None,
            scene: None,
            selected: None,
            dirty: false,
            play: None,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// 最小可见场景：一台相机 + 一个 quad，便于空编辑器直接看到视口内容。
    pub fn demo() -> Self {
        let mut world = World::new();
        let camera_transform = GlobalTransform {
            position: [0.0, 0.0, 5.0],
            ..Default::default()
        };
        world.spawn((
            Name {
                value: "camera".to_string(),
            },
            Camera::default(),
            camera_transform,
        ));
        let transform = Transform {
            scale: [2.0, 2.0, 2.0],
            ..Default::default()
        };
        world.spawn((
            transform,
            GlobalTransform::default(),
            MeshRenderer {
                model: "builtin://quad".to_string(),
            },
            Material {
                base_color: [0.3, 0.7, 1.0, 1.0],
                roughness: 0.5,
                ..Material::default()
            },
        ));
        Self::new(world)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let is_project = path.is_dir()
            || path
                .file_name()
                .is_some_and(|name| name == std::ffi::OsStr::new("project.xgc"));
        let (data, scene_path, project, scene) = if is_project {
            let session =
                ProjectSession::open(path).map_err(|err| format!("读取项目失败：{err}"))?;
            let scene = session.entry_scene().to_string();
            let scene_path = session
                .scene_path(&scene)
                .map_err(|err| format!("入口场景路径不合法：{err}"))?;
            let data = session
                .load_entry()
                .map_err(|err| format!("读取入口场景失败：{err}"))?;
            (data, scene_path, Some(session), Some(scene))
        } else {
            let data = load_world(path).map_err(|err| format!("读取世界失败：{err}"))?;
            (data, path.to_path_buf(), None, None)
        };
        let mut world = World::new();
        world
            .restore_world_data(&data)
            .map_err(|err| format!("恢复世界失败：{err}"))?;
        Ok(Self {
            world,
            registry: builtin_registry(),
            path: Some(scene_path),
            project,
            scene,
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

    /// 项目名；单场景打开时为 None。
    pub fn project_name(&self) -> Option<&str> {
        self.project
            .as_ref()
            .map(|session| session.manifest().name.as_str())
    }

    /// 项目根；单场景打开时为 None。Play Scripts 默认用它。
    pub fn project_root(&self) -> Option<&Path> {
        self.project.as_ref().map(ProjectSession::root)
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

    /// Play 帧的粒子四边形；编辑态没有运行时时钟，因此为空。
    pub fn particle_quads(&self) -> &[xgc_world::ParticleQuad] {
        self.play
            .as_ref()
            .map_or(&[], |session| session.particles.quads())
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

    pub fn camera_entity(&self) -> Option<EntityId> {
        let mut selected: Option<EntityId> = None;
        self.world.for_each::<Camera>(|key, camera| {
            if !camera.active {
                return;
            }
            let Some(entity) = self.world.entity(key) else {
                return;
            };
            if selected.is_none_or(|existing| entity < existing) {
                selected = Some(entity);
            }
        });
        selected
    }

    /// 轨道相机：yaw 绕 Y、pitch 抬升、distance 为半径；写 Transform 与 GlobalTransform。
    pub fn set_camera_orbit(&mut self, yaw: f32, pitch: f32, distance: f32) -> Result<(), String> {
        let id = self
            .camera_entity()
            .ok_or_else(|| "场景没有 active 相机".to_string())?;
        let key = self
            .world
            .key(id)
            .ok_or_else(|| format!("相机实体 {} 不存在", id.raw()))?;
        let distance = distance.max(0.1);
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        let position = [
            distance * sin_yaw * cos_pitch,
            -distance * sin_pitch,
            distance * cos_yaw * cos_pitch,
        ];
        let rotation = quat_mul(
            [0.0, sin_yaw * 0.5, 0.0, cos_yaw * 0.5],
            [sin_pitch * 0.5, 0.0, 0.0, cos_pitch * 0.5],
        );
        if let Some(transform) = self.world.get_mut::<Transform>(key) {
            transform.position = position;
            transform.rotation = rotation;
        }
        if let Some(transform) = self.world.get_mut::<GlobalTransform>(key) {
            transform.position = position;
            transform.rotation = rotation;
        }
        self.dirty = true;
        Ok(())
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

    /// 手动记录一个撤销点（Gizmo 拖拽开始时调用一次）。
    pub fn checkpoint(&mut self) {
        if self.play.is_none() {
            self.push_undo();
        }
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn push_undo(&mut self) {
        self.record_undo(self.world.world_data());
    }

    fn record_undo(&mut self, snapshot: WorldData) {
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
}

fn value_as_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        other => format!("{other:?}"),
    }
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}
