//! 按 `script.lua` 组件驱动脚本实体：每个实体一个 Lua 宿主，帧内产生命令后统一结算。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use xgc_asset::{AssetCatalog, AssetPaths, SearchPaths};
use xgc_base::{EntityId, EntityKey};
use xgc_world::{AudioCommand, AudioMixer, ListenerPose, Script, World};

use crate::view::{EntityView, ScriptView};
use crate::LuaHost;

struct Entry {
    host: LuaHost,
    source: PathBuf,
}

impl Entry {
    fn error_for(&self, entity: EntityId, result: Result<bool, String>) -> Option<String> {
        result
            .err()
            .map(|err| format!("{} 实体 {entity}：{err}", self.source.display()))
    }
}

pub struct ScriptRuntime {
    assets: AssetPaths,
    entries: HashMap<EntityId, Entry>,
    started: HashSet<EntityId>,
    view: Rc<RefCell<ScriptView>>,
    entity_keys: Vec<(EntityId, EntityKey)>,
    time: f32,
    /// 本帧第一处脚本错误。只打 stderr 不够：宿主（`run-scripts` / `trace`）要看得到
    /// 失败并退出 2，否则坏脚本会静默退化成"什么都没发生"。
    frame_error: Option<String>,
}

impl ScriptRuntime {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_assets(AssetPaths::new(root, SearchPaths::default(), None))
    }

    pub fn with_catalog(root: impl Into<PathBuf>, catalog: AssetCatalog) -> Self {
        Self::with_assets(AssetPaths::new(root, SearchPaths::default(), Some(catalog)))
    }

    pub fn with_assets(assets: AssetPaths) -> Self {
        Self {
            assets,
            entries: HashMap::new(),
            started: HashSet::new(),
            view: Rc::new(RefCell::new(ScriptView::default())),
            entity_keys: Vec::new(),
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
        let mut stale: Vec<EntityId> = self
            .entries
            .keys()
            .filter(|entity| !live.contains(entity))
            .copied()
            .collect();
        stale.sort_unstable();
        for entity in stale {
            if let Some(entry) = self.entries.remove(&entity) {
                if let Some(err) = entry.error_for(entity, entry.host.destroy()) {
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
            let entry = self.entries.get(&entity).expect("刚插入");
            let start_error = if started {
                entry.error_for(entity, entry.host.start_legacy())
            } else {
                None
            };
            let update_error = entry.error_for(entity, entry.host.update_legacy(dt));
            let fixed_error = entry.error_for(entity, entry.host.fixed_legacy());
            pending.extend(entry.host.take_commands());
            if let Some(err) = start_error {
                self.note_error(&err);
            }
            if let Some(err) = update_error {
                self.note_error(&err);
            }
            if let Some(err) = fixed_error {
                self.note_error(&err);
            }
        }
        // 碰撞事件由物理系统在本步写入；宿主阶段在所有系统之后，所以这里看到的就是本步
        // 的接触。先于命令结算派发：钩子写的命令与 update / fixed_update 写的命令同批结算。
        let collisions: Vec<_> = world
            .collision_events()
            .iter()
            .map(|event| (event.entity, event.other, event.enter))
            .collect();
        for (entity, other, enter) in collisions {
            let error = self.entries.get(&entity).and_then(|entry| {
                let error = entry.error_for(entity, entry.host.collision_legacy(enter, other));
                pending.extend(entry.host.take_commands());
                error
            });
            if let Some(err) = error {
                self.note_error(&err);
            }
        }
        // 动画事件由 `AnimatorSystem` 在上一步末尾写入（动画在宿主之后）；宿主
        // 在这里消费，所以脚本看到的是一步之前跨过的事件——CLI / 编辑器 Play
        // 的固定步是 60 Hz，差一帧不影响"按事件名做反应"的语义。
        let anim_events: Vec<_> = world
            .animation_events()
            .iter()
            .map(|event| (event.entity, event.name.clone()))
            .collect();
        for (entity, name) in anim_events {
            let error = self.entries.get(&entity).and_then(|entry| {
                let error = entry.error_for(entity, entry.host.anim_event_legacy(&name));
                pending.extend(entry.host.take_commands());
                error
            });
            if let Some(err) = error {
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

    /// 帧末结算播放命令，保留队列容量。脚本设置的音量覆盖作者缺省值。
    pub fn finish_audio_frame(
        &mut self,
        authored_mix: AudioMixer,
        consume: impl FnOnce(AudioMixer, Option<ListenerPose>, std::vec::Drain<'_, AudioCommand>),
    ) {
        let mut view = self.view.borrow_mut();
        let audio = &mut view.audio;
        consume(
            audio.mix(authored_mix),
            audio.listener,
            audio.commands.drain(..),
        );
    }

    fn note_error(&mut self, err: &str) {
        eprintln!("script: {err}");
        if self.frame_error.is_none() {
            self.frame_error = Some(err.to_string());
        }
    }

    fn load_entity(&self, entity: EntityId, path: &str) -> Result<Entry, String> {
        let resolved = self
            .assets
            .resolve(path)
            .map_err(|err| format!("读取脚本 {path} 失败：{err}"))?;
        let source = std::fs::read_to_string(&resolved)
            .map_err(|err| format!("读取 {} 失败：{err}", resolved.display()))?;
        let host = LuaHost::with_commands_and_view(
            Rc::new(RefCell::new(Vec::new())),
            Rc::clone(&self.view),
            entity.raw(),
        )?;
        host.load_script(path, &source)?;
        Ok(Entry {
            host,
            source: resolved,
        })
    }

    fn refresh_view(&mut self, world: &World, dt: f32) {
        self.entity_keys.clear();
        self.entity_keys.reserve(world.len());
        self.entity_keys.extend(world.entity_keys());
        // 同名实体按最小持久 ID 首占；回收运行时槽位后这个顺序仍需保持。
        self.entity_keys.sort_unstable_by_key(|&(id, _)| id);

        let mut view = self.view.borrow_mut();
        view.delta = dt;
        view.time = self.time;
        view.entities
            .resize_with(self.entity_keys.len(), EntityView::default);
        for (row, &(id, key)) in view.entities.iter_mut().zip(&self.entity_keys) {
            row.refresh(world, id, key);
        }
        view.spawn_cursor = world.next_entity_id().raw();
        view.pending_spawns.clear();
        view.audio.authored_mix = world.audio_mixer();
        // 脚本帧失败或独立脚本调用方未消费时，旧请求不能漏进下一帧。
        view.audio.commands.clear();
    }

    /// 重新读取磁盘并执行所有已加载脚本；场景脚本热重载入口。
    pub fn reload_all(&mut self) -> Result<(), String> {
        let mut entities: Vec<_> = self.entries.keys().copied().collect();
        entities.sort_unstable();
        for entity in entities {
            let entry = &self.entries[&entity];
            let source = std::fs::read_to_string(&entry.source)
                .map_err(|err| format!("读取 {} 失败：{err}", entry.source.display()))?;
            entry
                .host
                .load_script(&entry.source.to_string_lossy(), &source)?;
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

    /// 外部输入桥：设置模拟量轴，脚本 `axis` 下一帧可见。
    pub fn set_axis(&mut self, axis: &str, value: f32) {
        self.view.borrow_mut().axes.insert(axis.to_string(), value);
    }

    pub fn clear_axes(&mut self) {
        self.view.borrow_mut().axes.clear();
    }
}
