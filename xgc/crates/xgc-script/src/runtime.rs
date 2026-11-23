//! 按 `script.lua` 组件驱动脚本实体：每个实体一个 Lua 宿主，帧内产生命令后统一结算。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use xgc_asset::AssetCatalog;
use xgc_base::EntityId;
use xgc_world::{Command, Script, Value, World};

use crate::legacy::{EntityView, ScriptView};
use crate::view::UiElementView;
use crate::LuaHost;

struct Entry {
    host: LuaHost,
    commands: Rc<RefCell<Vec<Command>>>,
    source: PathBuf,
}

pub struct ScriptRuntime {
    root: PathBuf,
    catalog: Option<AssetCatalog>,
    entries: HashMap<EntityId, Entry>,
    started: HashSet<EntityId>,
    view: Rc<RefCell<ScriptView>>,
    time: f32,
    /// 本帧第一处脚本错误。只打 stderr 不够：宿主（`run-scripts` / `trace`）要看得到
    /// 失败并退出 2，否则坏脚本会静默退化成"什么都没发生"。
    frame_error: Option<String>,
}

impl ScriptRuntime {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            catalog: None,
            entries: HashMap::new(),
            started: HashSet::new(),
            view: Rc::new(RefCell::new(ScriptView::default())),
            time: 0.0,
            frame_error: None,
        }
    }

    /// 旧场景脚本路径可能是 `guid://`；传资产目录后按 guid 解析。
    pub fn with_catalog(root: impl Into<PathBuf>, catalog: AssetCatalog) -> Self {
        Self {
            root: root.into(),
            catalog: Some(catalog),
            entries: HashMap::new(),
            started: HashSet::new(),
            view: Rc::new(RefCell::new(ScriptView::default())),
            time: 0.0,
            frame_error: None,
        }
    }

    /// 执行一帧脚本；脚本写入的命令在帧末统一 plan / apply。
    ///
    /// 单帧内第一处错误记在 `frame_error` 里，由 [`Self::take_error`] 取走：命令行的
    /// trace / run-scripts 需要据此退出 2，不能只靠 stderr。
    pub fn run_frame(&mut self, world: &mut World, dt: f32) {
        self.frame_error = None;
        self.time += dt;
        self.refresh_view(world, dt);
        let mut scripts = Vec::new();
        world.for_each::<Script>(|key, script| {
            if let Some(entity) = world.entity(key) {
                scripts.push((entity, script.path.clone()));
            }
        });
        scripts.sort_by_key(|(entity, _)| *entity);
        let live: HashSet<EntityId> = scripts.iter().map(|(entity, _)| *entity).collect();
        let stale: Vec<EntityId> = self
            .entries
            .keys()
            .filter(|entity| !live.contains(entity))
            .copied()
            .collect();
        for entity in stale {
            if let Some(entry) = self.entries.remove(&entity) {
                if let Err(err) = entry.host.destroy() {
                    self.note_error(&err);
                }
            }
            self.started.remove(&entity);
        }
        let mut pending = Vec::new();
        for (entity, path) in scripts {
            if !self.entries.contains_key(&entity) {
                match self.load_entity(entity, &path) {
                    Ok(entry) => {
                        self.entries.insert(entity, entry);
                    }
                    Err(err) => {
                        self.note_error(&err);
                        continue;
                    }
                }
            }
            let started = self.started.insert(entity);
            let entry = self.entries.get_mut(&entity).expect("刚插入");
            let start_error = if started {
                entry.host.start_legacy().err()
            } else {
                None
            };
            let update_error = entry.host.update_legacy(dt).err();
            pending.extend(entry.host.take_commands());
            if let Some(err) = start_error {
                self.note_error(&err);
            }
            if let Some(err) = update_error {
                self.note_error(&err);
            }
        }
        if pending.is_empty() {
            return;
        }
        let plan = world.plan(pending);
        if plan.is_valid() {
            if let Err(diagnostics) = world.apply(&plan) {
                self.note_error(&format!("命令应用失败：{diagnostics:?}"));
            }
        } else {
            self.note_error(&format!("命令计划无效：{:?}", plan.diagnostics));
        }
    }

    /// 取走本帧第一处脚本错误（没有则 `None`）。
    pub fn take_error(&mut self) -> Option<String> {
        self.frame_error.take()
    }

    fn note_error(&mut self, err: &str) {
        eprintln!("script: {err}");
        if self.frame_error.is_none() {
            self.frame_error = Some(err.to_string());
        }
    }

    fn load_entity(&self, entity: EntityId, path: &str) -> Result<Entry, String> {
        let resolved = self.resolve(path)?;
        let source = std::fs::read_to_string(&resolved)
            .map_err(|err| format!("读取 {} 失败：{err}", resolved.display()))?;
        let commands = Rc::new(RefCell::new(Vec::new()));
        let host = LuaHost::with_commands_and_view(
            Rc::clone(&commands),
            Rc::clone(&self.view),
            entity.raw(),
        )?;
        host.load_script(path, &source)?;
        Ok(Entry {
            host,
            commands,
            source: resolved,
        })
    }

    fn refresh_view(&self, world: &World, dt: f32) {
        let data = world.world_data();
        let mut entities = Vec::with_capacity(data.entities.len());
        for entity in &data.entities {
            let mut row = EntityView {
                id: entity.id.raw(),
                rotation: [0.0, 0.0, 0.0, 1.0],
                ..Default::default()
            };
            for component in &entity.components {
                match component.component.0 {
                    0x0101 => {
                        for (field, value) in &component.fields {
                            match (field.as_str(), value) {
                                ("position", Value::Vec3(value)) => {
                                    row.position =
                                        [value[0] as f32, value[1] as f32, value[2] as f32];
                                }
                                ("rotation", Value::Quat(value)) => {
                                    row.rotation = [
                                        value[0] as f32,
                                        value[1] as f32,
                                        value[2] as f32,
                                        value[3] as f32,
                                    ];
                                }
                                _ => {}
                            }
                        }
                    }
                    0x0102 => {
                        for (field, value) in &component.fields {
                            if let ("linear", Value::Vec3(value)) = (field.as_str(), value) {
                                row.velocity = [value[0] as f32, value[1] as f32, value[2] as f32];
                            }
                        }
                    }
                    0x0103 => {
                        for (field, value) in &component.fields {
                            if let ("value", Value::String(value)) = (field.as_str(), value) {
                                row.name = value.clone();
                            }
                        }
                    }
                    0x0506 => {
                        for (field, value) in &component.fields {
                            if let ("layers", Value::String(value)) = (field.as_str(), value) {
                                row.anim_layers = Some(value.clone());
                            }
                        }
                    }
                    0x0505 => {
                        for (field, value) in &component.fields {
                            if let ("parameters", Value::String(value)) = (field.as_str(), value) {
                                row.anim_parameters = Some(value.clone());
                            }
                        }
                    }
                    0x0601 => {
                        let mut ui = UiElementView {
                            visible: true,
                            interactable: true,
                            ..Default::default()
                        };
                        for (field, value) in &component.fields {
                            match (field.as_str(), value) {
                                ("anchor_min", Value::Vec2(value)) => {
                                    ui.anchor_min = [value[0] as f32, value[1] as f32];
                                }
                                ("anchor_max", Value::Vec2(value)) => {
                                    ui.anchor_max = [value[0] as f32, value[1] as f32];
                                }
                                ("offset_min", Value::Vec2(value)) => {
                                    ui.offset_min = [value[0] as f32, value[1] as f32];
                                }
                                ("offset_max", Value::Vec2(value)) => {
                                    ui.offset_max = [value[0] as f32, value[1] as f32];
                                }
                                ("text", Value::String(value)) => ui.text = value.clone(),
                                ("visible", Value::Bool(value)) => ui.visible = *value,
                                ("interactable", Value::Bool(value)) => ui.interactable = *value,
                                _ => {}
                            }
                        }
                        row.ui = Some(ui);
                    }
                    0x0603 => {
                        for (field, value) in &component.fields {
                            if let Value::Bool(value) = value {
                                match field.as_str() {
                                    "clicked" => row.clicked = *value,
                                    "hovered" => row.hovered = *value,
                                    "pressed" => row.pressed = *value,
                                    _ => {}
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            entities.push(row);
        }
        let mut view = self.view.borrow_mut();
        view.delta = dt;
        view.time = self.time;
        view.entities = entities;
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        if path.starts_with("guid://") {
            let catalog = self
                .catalog
                .as_ref()
                .ok_or_else(|| format!("{path} 需要资产目录"))?;
            return catalog
                .resolve(path)
                .map(Path::to_path_buf)
                .ok_or_else(|| format!("资产目录没有 {path}"));
        }
        Ok(self.root.join(path))
    }

    /// 重新读取磁盘并执行所有已加载脚本；场景脚本热重载入口。
    pub fn reload_all(&mut self) -> Result<(), String> {
        for entry in self.entries.values() {
            let source = std::fs::read_to_string(&entry.source)
                .map_err(|err| format!("读取 {} 失败：{err}", entry.source.display()))?;
            entry
                .host
                .load_script(&entry.source.to_string_lossy(), &source)?;
            let _ = &entry.commands;
        }
        Ok(())
    }

    pub fn entity_count(&self) -> usize {
        self.entries.len()
    }

    /// 外部输入桥：设置动作按住状态，脚本 `action_pressed` 下一帧可见。
    pub fn set_action(&mut self, action: &str, pressed: bool) {
        self.view
            .borrow_mut()
            .actions
            .insert(action.to_string(), pressed);
    }

    pub fn clear_actions(&mut self) {
        self.view.borrow_mut().actions.clear();
    }
}
