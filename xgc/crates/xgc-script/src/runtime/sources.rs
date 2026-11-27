//! 同一资产只建立一份批模块。已有全局钩子的挂载仍由各自 VM 管理生命周期。

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use mlua::Table;
use xgc_asset::AssetPaths;
use xgc_base::EntityId;
use xgc_world::{Command, World};

use crate::batch::BatchModule;
use crate::legacy::Commands;
use crate::module::PreparedModule;
use crate::{sandbox, LuaHost, ScriptView};

use super::note_error;

pub(super) enum MountVm {
    Entity(Box<MountedVm>),
    Batch(Rc<Path>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SourceStamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl From<Metadata> for SourceStamp {
    fn from(metadata: Metadata) -> Self {
        Self {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        }
    }
}

pub(super) struct MountedVm {
    pub(super) host: LuaHost,
    source: PathBuf,
    active: Option<SourceStamp>,
    observed: Option<SourceStamp>,
}

pub(super) struct Reload {
    module: PreparedModule,
    stamp: Option<SourceStamp>,
}

impl MountedVm {
    pub(super) fn observe(&mut self) -> bool {
        let current = fs::metadata(&self.source)
            .ok()
            .filter(Metadata::is_file)
            .map(SourceStamp::from);
        let changed = self.observed != current;
        self.observed = current;
        changed
    }

    pub(super) fn prepare_reload(
        &self,
        name: &str,
        changed_only: bool,
        view: &Rc<RefCell<ScriptView>>,
        commands: &Commands,
        batch_base: Option<&Table>,
    ) -> Result<Option<Reload>, String> {
        if changed_only && self.active == self.observed {
            return Ok(None);
        }
        let (source, stamp) = read_source(&self.source)?;
        // 初始化构造候选模块；步与步之间的世界和音频写入都要撤销。
        let module = initialize(
            view,
            commands,
            |_| false,
            || self.host.prepare_mount(name, &source, batch_base),
        )?;
        if batch_base.is_some() && module.batch_update.is_none() {
            return Err("切换到逐实体钩子后需要重新进入播放".into());
        }
        Ok(Some(Reload { module, stamp }))
    }

    pub(super) fn commit_reload(&mut self, reload: Reload) {
        self.host.commit_module(reload.module);
        self.active = reload.stamp;
        self.observed = reload.stamp;
    }
}

pub(super) struct ActiveBatch {
    vm: MountedVm,
    module: BatchModule,
    base: Table,
    commands: Commands,
}

#[derive(Default)]
pub(super) struct BatchSources {
    active: BTreeMap<Rc<Path>, ActiveBatch>,
    aliases: BTreeMap<String, Rc<Path>>,
    entity_paths: BTreeSet<PathBuf>,
}

impl BatchSources {
    pub(super) fn load(
        &mut self,
        assets: &AssetPaths,
        view: &Rc<RefCell<ScriptView>>,
        entity: EntityId,
        name: &str,
        commands: &Commands,
    ) -> Result<MountVm, String> {
        if let Some(path) = self.aliases.get(name) {
            return Ok(MountVm::Batch(Rc::clone(path)));
        }
        let resolved = assets.resolve(name).map_err(|err| err.to_string())?;
        let known_entity = self.entity_paths.contains(&resolved);
        let canonical = if known_entity {
            None
        } else {
            Some(fs::canonicalize(&resolved).map_err(|err| format!("定位 {name} 失败：{err}"))?)
        };
        if let Some(path) = canonical.as_deref() {
            if let Some((path, _)) = self.active.get_key_value(path) {
                self.aliases.insert(name.to_owned(), Rc::clone(path));
                return Ok(MountVm::Batch(Rc::clone(path)));
            }
        }
        let (source, stamp) = read_source(&resolved)?;
        let mut host = LuaHost::new()?;
        let detect = !known_entity
            && canonical
                .as_ref()
                .is_none_or(|path| !self.entity_paths.contains(path));
        let base = detect
            .then(|| sandbox::globals_before_mount(&host.lua))
            .transpose()
            .map_err(|err| err.to_string())?;
        host.register_mounted(Rc::clone(commands), Rc::clone(view), entity.raw())?;
        let prepared = initialize(
            view,
            commands,
            |module: &PreparedModule| module.batch_update.is_none(),
            || host.prepare_mount(name, &source, base.as_ref()),
        )?;
        let update = prepared.batch_update.clone();
        host.commit_module(prepared);
        let vm = MountedVm {
            host,
            source: resolved,
            active: stamp,
            observed: stamp,
        };
        if let Some(update) = update {
            let path = Rc::<Path>::from(canonical.expect("批脚本已解析实际路径"));
            let module = BatchModule::new(name.to_owned(), &vm.host, update)?;
            self.aliases.insert(name.to_owned(), Rc::clone(&path));
            self.active.insert(
                Rc::clone(&path),
                ActiveBatch {
                    vm,
                    module,
                    base: base.expect("批脚本已隔离挂载绑定"),
                    commands: Rc::clone(commands),
                },
            );
            return Ok(MountVm::Batch(path));
        }
        if let Some(path) = canonical {
            self.entity_paths.insert(path);
            self.entity_paths.insert(vm.source.clone());
        }
        Ok(MountVm::Entity(Box::new(vm)))
    }

    pub(super) fn begin_frame(&mut self) {
        for batch in self.active.values_mut() {
            batch.module.members.clear();
        }
    }

    pub(super) fn add_member(&mut self, path: &Path, entity: EntityId) {
        let members = &mut self
            .active
            .get_mut(path)
            .expect("批挂载的资产已加载")
            .module
            .members;
        if members.last() != Some(&entity) {
            members.push(entity);
        }
    }

    pub(super) fn run(
        &mut self,
        world: &World,
        commands: &mut Vec<Command>,
        error: &mut Option<String>,
    ) {
        let before = self.active.len();
        self.active.retain(|_, batch| {
            if batch.module.members.is_empty() {
                return false;
            }
            if let Err(err) = batch.module.run(&batch.vm.host, world, commands) {
                note_error(error, err);
            }
            true
        });
        if self.active.len() != before {
            self.aliases
                .retain(|_, path| self.active.contains_key(path));
        }
    }

    pub(super) fn observe(&mut self) -> bool {
        let mut changed = false;
        for batch in self.active.values_mut() {
            changed |= batch.vm.observe();
        }
        changed
    }

    /// 候选均成功之前不切换批回调或逐实体环境。
    pub(super) fn prepare_reloads(
        &mut self,
        changed_only: bool,
        view: &Rc<RefCell<ScriptView>>,
    ) -> Result<Vec<(&mut ActiveBatch, Reload)>, String> {
        let mut pending = Vec::new();
        for batch in self.active.values_mut() {
            let name = &batch.module.name;
            if let Some(reload) = batch
                .vm
                .prepare_reload(name, changed_only, view, &batch.commands, Some(&batch.base))
                .map_err(|err| format!("批脚本 {name}：{err}"))?
            {
                pending.push((batch, reload));
            }
        }
        Ok(pending)
    }
}

impl ActiveBatch {
    pub(super) fn commit_reload(&mut self, mut reload: Reload) -> usize {
        self.module.update = reload.module.batch_update.take().expect("候选仍使用批回调");
        self.vm.commit_reload(reload);
        self.module.members.len()
    }
}

fn read_source(path: &Path) -> Result<(String, Option<SourceStamp>), String> {
    let error = |err| format!("读取 {} 失败：{err}", path.display());
    let mut file = fs::File::open(path).map_err(error)?;
    // 先取已打开文件的版本，原子替换后的新文件不能被标记成已编译。
    let stamp = file.metadata().ok().map(SourceStamp::from);
    let mut source = String::new();
    file.read_to_string(&mut source).map_err(error)?;
    Ok((source, stamp))
}

fn initialize<T>(
    view: &Rc<RefCell<ScriptView>>,
    commands: &Commands,
    keep_effects: impl FnOnce(&T) -> bool,
    load: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    // UI setter 可能原地覆盖命令。候选失败后恢复此前待结算的内容。
    let previous = commands.borrow().clone();
    let mut keep = false;
    let result = ScriptView::initialize(
        view,
        |value| {
            keep = keep_effects(value);
            keep
        },
        load,
    );
    if !keep {
        *commands.borrow_mut() = previous;
    }
    result
}
