//! 按固定顺序派发 Lua 挂载与模块批回调，帧末统一结算命令。

mod sources;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use xgc_asset::{AssetCatalog, AssetPaths, SearchPaths};
use xgc_base::{EntityId, EntityKey};
use xgc_world::{AudioCommand, AudioMixer, Command, ListenerPose, Script, World};

use crate::legacy::Commands;
use crate::view::ScriptView;
use crate::LuaHost;
use sources::{BatchSources, MountVm};

struct Mount {
    path: String,
    vm: Option<MountVm>,
}

impl Mount {
    fn dispatch(
        &self,
        entity: EntityId,
        hook: impl FnOnce(&LuaHost) -> Result<bool, String>,
        frame_error: &mut Option<String>,
    ) {
        if let Some(MountVm::Entity(vm)) = &self.vm {
            if let Err(err) = hook(&vm.host) {
                note_error(frame_error, mount_error(&self.path, entity, err));
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

    fn prepare_mounts(
        &mut self,
        assets: &AssetPaths,
        view: &Rc<RefCell<ScriptView>>,
        batches: &mut BatchSources,
        entity: EntityId,
        error: &mut Option<String>,
    ) {
        for mount in &mut self.mounts {
            if mount.vm.is_none() {
                match batches.load(assets, view, entity, &mount.path, &self.commands) {
                    Ok(vm) => mount.vm = Some(vm),
                    Err(err) => note_error(error, mount_error(&mount.path, entity, err)),
                }
            }
            if let Some(MountVm::Batch(path)) = &mount.vm {
                batches.add_member(path, entity);
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
    batches: BatchSources,
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
            batches: BatchSources::default(),
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
        self.batches.begin_frame();
        self.entries.retain(|&entity, scripts| {
            if self
                .script_keys
                .binary_search_by_key(&entity, |&(id, _)| id)
                .is_ok()
            {
                return true;
            }
            // 挂载已退出，现在只收销毁钩子的写入。
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
            scripts.prepare_mounts(&self.assets, &self.view, &mut self.batches, entity, error);
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
        // 批回调读本步物理完成后的世界，成功写入排在同一步的挂载钩子之后。
        self.batches.run(world, &mut pending, error);
        self.discard_action_edges();
        let mut view = self.view.borrow_mut();
        let body_velocities = view.body_velocities.drain(..);
        if !pending.is_empty() {
            let plan = world.plan(pending);
            if !plan.is_valid() {
                note_error(
                    &mut self.frame_error,
                    format!("命令计划无效：{:?}", plan.diagnostics),
                );
                return;
            }
            if let Err(diagnostics) = world.apply(&plan) {
                note_error(
                    &mut self.frame_error,
                    format!("命令应用失败：{diagnostics:?}"),
                );
                return;
            }
        }
        for (entity, linear) in body_velocities {
            world.queue_body_velocity(entity, linear);
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

        let mut view = self.view.borrow_mut();
        view.delta = dt;
        view.time = self.time;
        view.refresh_entities(world, &mut self.entity_keys);
        view.spawn_cursor = world.next_entity_id().raw();
        view.pending_spawns.clear();
        view.audio.authored_mix = world.audio_mixer();
        // 脚本帧失败或独立脚本调用方未消费时，旧请求不能漏进下一帧。
        view.audio.commands.clear();
    }

    /// 重新读取所有挂载。候选模块均通过初始化后再替换活动代码，运行期状态留在原 VM。
    pub fn reload_all(&mut self) -> Result<(), String> {
        self.reload_modules(false).map(|_| ())
    }

    /// 仅由编辑器定时调用。失败后记住已看到的版本，下次保存时连同积压的修改一起重试。
    pub fn poll_reload(&mut self) -> Result<usize, String> {
        let mut changed = self.batches.observe();
        for mount in self
            .entries
            .values_mut()
            .flat_map(|scripts| &mut scripts.mounts)
        {
            if let Some(MountVm::Entity(vm)) = &mut mount.vm {
                changed |= vm.observe();
            }
        }
        if changed {
            self.reload_modules(true)
        } else {
            Ok(0)
        }
    }

    fn reload_modules(&mut self, changed_only: bool) -> Result<usize, String> {
        let mut modules = Vec::new();
        for (&entity, scripts) in &mut self.entries {
            for Mount { path, vm } in &mut scripts.mounts {
                let Some(MountVm::Entity(vm)) = vm else {
                    continue;
                };
                if let Some(candidate) = vm
                    .prepare_reload(path, changed_only, &self.view, &scripts.commands, None)
                    .map_err(|err| mount_error(path, entity, err))?
                {
                    modules.push((vm, candidate));
                }
            }
        }
        let batches = self.batches.prepare_reloads(changed_only, &self.view)?;
        let mut count = modules.len();
        for (vm, candidate) in modules {
            vm.commit_reload(candidate);
        }
        for (batch, candidate) in batches {
            count += batch.commit_reload(candidate);
        }
        Ok(count)
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

fn mount_error(path: &str, entity: EntityId, err: String) -> String {
    format!("脚本 {path} 实体 {entity}：{err}")
}

fn note_error(frame_error: &mut Option<String>, err: String) {
    eprintln!("script: {err}");
    frame_error.get_or_insert(err);
}
