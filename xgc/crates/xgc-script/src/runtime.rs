//! 按 `script.lua` 组件驱动脚本实体：每个实体一个 Lua 宿主，帧内产生命令后统一结算。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use xgc_asset::AssetCatalog;
use xgc_base::EntityId;
use xgc_world::{Command, Script, World};

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
}

impl ScriptRuntime {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            catalog: None,
            entries: HashMap::new(),
            started: HashSet::new(),
        }
    }

    /// 旧场景脚本路径可能是 `guid://`；传资产目录后按 guid 解析。
    pub fn with_catalog(root: impl Into<PathBuf>, catalog: AssetCatalog) -> Self {
        Self {
            root: root.into(),
            catalog: Some(catalog),
            entries: HashMap::new(),
            started: HashSet::new(),
        }
    }

    /// 执行一帧脚本；脚本写入的命令在帧末统一 plan / apply。
    pub fn run_frame(&mut self, world: &mut World, dt: f32) {
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
                    eprintln!("script: {err}");
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
                        eprintln!("script: {err}");
                        continue;
                    }
                }
            }
            let entry = self.entries.get_mut(&entity).expect("刚插入");
            if self.started.insert(entity) {
                if let Err(err) = entry.host.start() {
                    eprintln!("script: {err}");
                }
            }
            if let Err(err) = entry.host.update(dt) {
                eprintln!("script: {err}");
            }
            pending.extend(entry.host.take_commands());
        }
        if pending.is_empty() {
            return;
        }
        let plan = world.plan(pending);
        if plan.is_valid() {
            if let Err(diagnostics) = world.apply(&plan) {
                eprintln!("script: 命令应用失败：{diagnostics:?}");
            }
        } else {
            eprintln!("script: 命令计划无效：{:?}", plan.diagnostics);
        }
    }

    fn load_entity(&self, entity: EntityId, path: &str) -> Result<Entry, String> {
        let resolved = self.resolve(path)?;
        let source = std::fs::read_to_string(&resolved)
            .map_err(|err| format!("读取 {} 失败：{err}", resolved.display()))?;
        let commands = Rc::new(RefCell::new(Vec::new()));
        let host = LuaHost::with_commands(Rc::clone(&commands))?;
        host.load_script(path, &source)?;
        let _ = entity;
        Ok(Entry {
            host,
            commands,
            source: resolved,
        })
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
}
