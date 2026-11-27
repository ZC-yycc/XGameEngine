//! 按 `script.lua.paths` 顺序驱动每个实体的 Lua 挂载，帧末统一结算命令。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use xgc_asset::{AssetCatalog, AssetPaths, SearchPaths};
use xgc_base::{EntityId, EntityKey};
use xgc_world::{AudioCommand, AudioMixer, Command, ListenerPose, Script, World};

use crate::legacy::Commands;
use crate::view::{EntityView, ScriptView};
use crate::LuaHost;

struct MountedVm {
    host: LuaHost,
    source: PathBuf,
}

struct Mount {
    path: String,
    vm: Option<MountedVm>,
}

impl Mount {
    fn error(&self, entity: EntityId, err: String) -> String {
        format!("脚本 {} 实体 {entity}：{err}", self.path)
    }

    fn dispatch(
        &self,
        entity: EntityId,
        hook: impl FnOnce(&LuaHost) -> Result<bool, String>,
        frame_error: &mut Option<String>,
    ) {
        if let Some(vm) = &self.vm {
            if let Err(err) = hook(&vm.host) {
                note_error(frame_error, self.error(entity, err));
            }
        }
    }
}

#[derive(Default)]
struct EntityScripts {
    mounts: Vec<Mount>,
    // 同一实体的挂载共享待结算写入，后执行的 UI 调用才能读到先前挂载修改的字段。
    commands: Commands,
}

impl EntityScripts {
    fn reconcile(&mut self, entity: EntityId, script: &Script, error: &mut Option<String>) {
        let paths = mounted_paths(script);
        if self
            .mounts
            .iter()
            .map(|mount| mount.path.as_str())
            .eq(paths.clone())
        {
            return;
        }

        // 相同路径按出现顺序认回旧 VM。先取到的副本始终优先保留。
        let mut previous = std::mem::take(&mut self.mounts);
        self.mounts.extend(paths.map(|path| {
            if let Some(index) = previous.iter().position(|mount| mount.path == path) {
                previous.remove(index)
            } else {
                Mount {
                    path: path.to_owned(),
                    vm: None,
                }
            }
        }));
        for mount in previous {
            mount.dispatch(entity, LuaHost::destroy, error);
        }
    }

    fn load_missing(
        &mut self,
        assets: &AssetPaths,
        view: &Rc<RefCell<ScriptView>>,
        entity: EntityId,
        error: &mut Option<String>,
    ) {
        for mount in &mut self.mounts {
            if mount.vm.is_some() {
                continue;
            }
            match load_mount(assets, view, entity, &mount.path, &self.commands) {
                Ok(vm) => mount.vm = Some(vm),
                Err(err) => note_error(error, mount.error(entity, err)),
            }
        }
    }

    fn dispatch(
        &self,
        entity: EntityId,
        hook: impl Fn(&LuaHost) -> Result<bool, String>,
        error: &mut Option<String>,
    ) {
        for mount in &self.mounts {
            mount.dispatch(entity, &hook, error);
        }
    }

    fn drain_into(&self, pending: &mut Vec<Command>) {
        pending.append(&mut self.commands.borrow_mut());
    }
}

pub struct ScriptRuntime {
    assets: AssetPaths,
    entries: BTreeMap<EntityId, EntityScripts>,
    view: Rc<RefCell<ScriptView>>,
    entity_keys: Vec<(EntityId, EntityKey)>,
    script_keys: Vec<(EntityId, EntityKey)>,
    time: f32,
    /// 本帧第一处脚本错误。宿主（`run-scripts` / `trace`）据此标记帧失败。
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
            entries: BTreeMap::new(),
            view: Rc::new(RefCell::new(ScriptView::default())),
            entity_keys: Vec::new(),
            script_keys: Vec::new(),
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
        self.script_keys.clear();
        world.for_each::<Script>(|key, script| {
            if mounted_paths(script).next().is_some() {
                if let Some(entity) = world.entity(key) {
                    self.script_keys.push((entity, key));
                }
            }
        });
        self.script_keys.sort_unstable_by_key(|&(id, _)| id);

        let mut pending = Vec::new();
        let error = &mut self.frame_error;
        self.entries.retain(|&entity, scripts| {
            if self
                .script_keys
                .binary_search_by_key(&entity, |&(id, _)| id)
                .is_ok()
            {
                return true;
            }
            // 热重载后尚未结算的旧命令与挂载一起失效；现在只收销毁钩子的写入。
            scripts.commands.borrow_mut().clear();
            scripts.dispatch(entity, LuaHost::destroy, error);
            if world.key(entity).is_none() {
                // 旧端已删除实体的自身 setter 是空操作，不能因此连带拒绝对其他实体的写入。
                scripts
                    .commands
                    .borrow_mut()
                    .retain(|command| match command {
                        Command::SetField { entity: target, .. }
                        | Command::AddComponent { entity: target, .. }
                        | Command::RemoveComponent { entity: target, .. }
                        | Command::Despawn { entity: target } => *target != entity,
                        Command::Replace { data } => data.id != entity,
                        Command::Spawn { .. } => true,
                    });
            }
            scripts.drain_into(&mut pending);
            false
        });
        for &(entity, key) in &self.script_keys {
            let script = world.get::<Script>(key).expect("脚本组件在派发期间只读");
            let scripts = self.entries.entry(entity).or_default();
            scripts.reconcile(entity, script, error);
            scripts.load_missing(&self.assets, &self.view, entity, error);
            // 保留 XGC 按实体派发的阶段；同一实体在每个阶段按挂载顺序调用。
            scripts.dispatch(entity, LuaHost::start_legacy, error);
            scripts.dispatch(entity, |host| host.update_legacy(dt), error);
            scripts.dispatch(entity, LuaHost::fixed_legacy, error);
            scripts.drain_into(&mut pending);
        }
        // 先派发本步物理碰撞，再派发上一步 Animator 排入的动画事件；事件写入同批结算。
        for event in world.collision_events() {
            if let Some(scripts) = self.entries.get(&event.entity) {
                scripts.dispatch(
                    event.entity,
                    |host| host.collision_legacy(event.enter, event.other),
                    error,
                );
                scripts.drain_into(&mut pending);
            }
        }
        for event in world.animation_events() {
            if let Some(scripts) = self.entries.get(&event.entity) {
                scripts.dispatch(
                    event.entity,
                    |host| host.anim_event_legacy(&event.name),
                    error,
                );
                scripts.drain_into(&mut pending);
            }
        }
        self.discard_action_edges();
        if pending.is_empty() {
            return;
        }
        let plan = world.plan(pending);
        if plan.is_valid() {
            if let Err(diagnostics) = world.apply(&plan) {
                note_error(
                    &mut self.frame_error,
                    format!("命令应用失败：{diagnostics:?}"),
                );
            }
        } else {
            note_error(
                &mut self.frame_error,
                format!("命令计划无效：{:?}", plan.diagnostics),
            );
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
        for (&entity, scripts) in &self.entries {
            for mount in &scripts.mounts {
                let Some(vm) = &mount.vm else {
                    continue;
                };
                let source = std::fs::read_to_string(&vm.source).map_err(|err| {
                    mount.error(entity, format!("读取 {} 失败：{err}", vm.source.display()))
                })?;
                vm.host
                    .load_script(&mount.path, &source)
                    .map_err(|err| mount.error(entity, err))?;
            }
        }
        Ok(())
    }

    pub fn entity_count(&self) -> usize {
        self.entries
            .values()
            .filter(|scripts| scripts.mounts.iter().any(|mount| mount.vm.is_some()))
            .count()
    }

    /// 已成功加载的挂载数，同一实体的多个 VM 分开计数。
    pub fn script_count(&self) -> usize {
        self.entries
            .values()
            .flat_map(|scripts| &scripts.mounts)
            .filter(|mount| mount.vm.is_some())
            .count()
    }

    /// 无设备边沿的调用方只需提交当前按住状态，转换会在下一帧消费。
    pub fn set_action(&mut self, action: &str, down: bool) {
        let mut view = self.view.borrow_mut();
        let state = view.action_mut(action);
        state.pressed |= down && !state.down;
        state.released |= !down && state.down;
        state.down = down;
    }

    /// 宿主已折叠逐个绑定的边沿，不能再从动作的按住状态推导一次。
    pub fn set_action_input(&mut self, action: &str, down: bool, pressed: bool, released: bool) {
        let mut view = self.view.borrow_mut();
        let state = view.action_mut(action);
        state.down = down;
        state.pressed |= pressed;
        state.released |= released;
    }

    pub fn discard_action_edges(&mut self) {
        for state in self.view.borrow_mut().actions.values_mut() {
            state.pressed = false;
            state.released = false;
        }
    }

    pub fn clear_actions(&mut self) {
        self.view.borrow_mut().actions.clear();
    }

    /// 外部输入桥：设置模拟量轴，脚本 `axis` 下一帧可见。
    pub fn set_axis(&mut self, axis: &str, value: f32) {
        let mut view = self.view.borrow_mut();
        if let Some(current) = view.axes.get_mut(axis) {
            *current = value;
        } else {
            view.axes.insert(axis.to_owned(), value);
        }
    }

    pub fn clear_axes(&mut self) {
        self.view.borrow_mut().axes.clear();
    }
}

fn mounted_paths(script: &Script) -> impl Iterator<Item = &str> + Clone {
    script
        .paths
        .iter()
        .map(String::as_str)
        .filter(|path| !path.is_empty())
}

fn load_mount(
    assets: &AssetPaths,
    view: &Rc<RefCell<ScriptView>>,
    entity: EntityId,
    path: &str,
    commands: &Commands,
) -> Result<MountedVm, String> {
    let resolved = assets.resolve(path).map_err(|err| err.to_string())?;
    let source = std::fs::read_to_string(&resolved)
        .map_err(|err| format!("读取 {} 失败：{err}", resolved.display()))?;
    let host = LuaHost::with_commands_and_view(Rc::clone(commands), Rc::clone(view), entity.raw())?;
    // 顶层代码可能改已有的 UI 写入。加载失败仍须保留其他挂载已经排队的命令。
    let previous = commands.borrow().clone();
    if let Err(err) = host.load_script(path, &source) {
        *commands.borrow_mut() = previous;
        return Err(err);
    }
    Ok(MountedVm {
        host,
        source: resolved,
    })
}

fn note_error(frame_error: &mut Option<String>, err: String) {
    eprintln!("script: {err}");
    frame_error.get_or_insert(err);
}
